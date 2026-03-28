defmodule Canary.Health.Checker do
  @moduledoc """
  GenServer per health check target. Owns probe scheduling (monotonic
  clock + jitter), in-memory state, and state machine transitions.
  Persists check results and enqueues webhook jobs on transitions.
  """

  use GenServer, restart: :transient

  alias Canary.Health.{Probe, SSRFGuard, StateMachine}
  alias Canary.{Incidents, Repo, Timeline}
  alias Canary.Schemas.{Target, TargetCheck, TargetState}

  require Logger

  defstruct [:target, :state, :counters, :jitter_seed]

  def start_link(%Target{} = target) do
    GenServer.start_link(__MODULE__, target, name: via(target.id))
  end

  def via(target_id), do: {:via, Registry, {Canary.Health.Registry, target_id}}

  def check_now(target_id) do
    GenServer.cast(via(target_id), :check_now)
  end

  # --- Callbacks ---

  @impl true
  def init(%Target{} = target) do
    Logger.metadata(target: target.name)

    state = load_persisted_state(target.id)

    counters = %{
      consecutive_failures: state.consecutive_failures,
      consecutive_successes: state.consecutive_successes,
      transitions: []
    }

    jitter_seed = :erlang.phash2(target.id)

    s = %__MODULE__{
      target: target,
      state: String.to_existing_atom(state.state),
      counters: counters,
      jitter_seed: jitter_seed
    }

    schedule_check(s)
    {:ok, s}
  end

  @impl true
  def handle_info(:check, %{target: target} = s) do
    s = do_check(target, s)
    schedule_check(s)
    {:noreply, s}
  end

  @impl true
  def handle_cast(:check_now, %{target: target} = s) do
    s = do_check(target, s)
    {:noreply, s}
  end

  @impl true
  def terminate(_reason, %{target: target, state: state, counters: counters}) do
    persist_state(target.id, state, counters, DateTime.utc_now() |> DateTime.to_iso8601())
  end

  # --- Internal ---

  defp do_check(target, s) do
    case SSRFGuard.validate_url(
           target.url,
           Application.get_env(:canary, :allow_private_targets, false)
         ) do
      :ok ->
        execute_probe(target, s)

      {:error, reason} ->
        Logger.warning("SSRF blocked: #{reason}", target: target.name)
        record_failure(target, s, "connection_error", reason)
    end
  end

  defp execute_probe(target, s) do
    event =
      case Probe.check(target) do
        {:ok, result} ->
          persist_check(target.id, result)
          :success

        {:error, result} ->
          persist_check(target.id, result)
          :failure
      end

    thresholds = %{
      degraded_after: target.degraded_after,
      down_after: target.down_after,
      up_after: target.up_after
    }

    {new_state, new_counters, effects} =
      StateMachine.transition(s.state, event, thresholds, s.counters)

    if new_state != s.state do
      Logger.info("#{target.name}: #{s.state} → #{new_state}")
    end

    process_effects(target, s.state, new_state, effects, new_counters)

    %{s | state: new_state, counters: new_counters}
  end

  defp record_failure(target, s, result, detail) do
    persist_check(target.id, %{
      status_code: nil,
      latency_ms: 0,
      result: result,
      tls_expires_at: nil,
      error_detail: detail
    })

    thresholds = %{
      degraded_after: target.degraded_after,
      down_after: target.down_after,
      up_after: target.up_after
    }

    {new_state, new_counters, effects} =
      StateMachine.transition(s.state, :failure, thresholds, s.counters)

    process_effects(target, s.state, new_state, effects, new_counters)

    %{s | state: new_state, counters: new_counters}
  end

  defp persist_check(target_id, result) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    %TargetCheck{}
    |> TargetCheck.changeset(%{
      target_id: target_id,
      checked_at: now,
      status_code: result.status_code,
      latency_ms: result.latency_ms,
      result: result.result,
      tls_expires_at: result.tls_expires_at,
      error_detail: result.error_detail
    })
    |> Repo.insert!()
  end

  defp persist_state(target_id, state, counters, now) do
    attrs = %{
      state: to_string(state),
      consecutive_failures: counters.consecutive_failures,
      consecutive_successes: counters.consecutive_successes,
      last_checked_at: now
    }

    attrs =
      if counters.consecutive_successes > 0 do
        Map.put(attrs, :last_success_at, now)
      else
        Map.put(attrs, :last_failure_at, now)
      end

    case Repo.get(TargetState, target_id) do
      nil ->
        %TargetState{target_id: target_id}
        |> TargetState.changeset(attrs)
        |> Repo.insert!()

      ts ->
        ts |> TargetState.changeset(attrs) |> Repo.update!()
    end
  end

  defp process_effects(target, old_state, new_state, effects, counters) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    case Enum.find(effects, fn
           {:webhook, _, _} -> true
           _ -> false
         end) do
      {:webhook, event_type, _meta} ->
        service = Target.service_name(target)
        event_name = webhook_event_name(event_type)

        case Repo.transaction(fn ->
               state = persist_state(target.id, new_state, counters, now)
               seq = state.sequence + 1

               state =
                 state
                 |> TargetState.changeset(%{
                   sequence: seq,
                   last_transition_at: now
                 })
                 |> Repo.update!()

               payload =
                 Timeline.record_health_transition!(
                   event_name,
                   target,
                   old_state,
                   new_state,
                   state,
                   seq,
                   now
                 )

               Canary.Workers.WebhookDelivery.enqueue_for_event(event_name, payload)
               payload
             end) do
          {:ok, _payload} ->
            case Incidents.correlate(:health_transition, target.id, service) do
              {:ok, _incident} ->
                :ok

              {:error, reason} ->
                Logger.error(
                  "Failed to correlate incident for target #{target.id}: #{correlation_error_tag(reason)}"
                )
            end

          {:error, reason} ->
            raise "failed to record health transition #{event_name}: #{inspect(reason)}"
        end

      _ ->
        persist_state(target.id, new_state, counters, now)
        :ok
    end
  end

  defp webhook_event_name(:health_check_degraded), do: "health_check.degraded"
  defp webhook_event_name(:health_check_down), do: "health_check.down"
  defp webhook_event_name(:health_check_recovered), do: "health_check.recovered"

  defp correlation_error_tag({:exception, module}) when is_atom(module),
    do: Atom.to_string(module)

  defp correlation_error_tag({kind, reason}), do: "#{kind}:#{correlation_error_tag(reason)}"
  defp correlation_error_tag(reason) when is_atom(reason), do: Atom.to_string(reason)
  defp correlation_error_tag(%module{}) when is_atom(module), do: Atom.to_string(module)
  defp correlation_error_tag(_reason), do: "unexpected"

  defp load_persisted_state(target_id) do
    case Repo.get(TargetState, target_id) do
      nil ->
        %TargetState{
          target_id: target_id,
          state: "unknown",
          consecutive_failures: 0,
          consecutive_successes: 0
        }

      ts ->
        ts
    end
  end

  defp schedule_check(%{target: target, jitter_seed: seed}) do
    jitter_range = div(target.interval_ms, 10)
    jitter = rem(seed + System.monotonic_time(:millisecond), jitter_range * 2) - jitter_range
    delay = max(target.interval_ms + jitter, 1_000)
    Process.send_after(self(), :check, delay)
  end
end

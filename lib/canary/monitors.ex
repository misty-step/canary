defmodule Canary.Monitors do
  @moduledoc """
  Non-HTTP health monitors driven by client check-ins instead of URL probes.
  """

  import Ecto.Query

  alias Canary.{CorrelationErrorTag, ID, IncidentCorrelation, Repo, Timeline}
  alias Canary.Schemas.{Monitor, MonitorCheckIn, MonitorState}

  require Logger

  @transition_states ~w(up degraded down)
  @success_statuses ~w(alive in_progress ok)
  @check_in_statuses @success_statuses ++ ["error"]

  @spec bootstrap() :: :ok
  def bootstrap do
    from(m in Monitor, select: m.id)
    |> Repo.all()
    |> Enum.each(&ensure_monitor_state/1)

    :ok
  end

  @spec list_monitors() :: [Monitor.t()]
  def list_monitors do
    from(m in Monitor, order_by: m.name)
    |> Canary.Repos.read_repo().all()
  end

  @spec add_monitor(map()) :: {:ok, Monitor.t()} | {:error, Ecto.Changeset.t()}
  def add_monitor(attrs) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    attrs =
      attrs
      |> Map.put("id", ID.monitor_id())
      |> Map.put("created_at", now)
      |> Map.put_new("service", Map.get(attrs, "name"))
      |> Map.put_new("grace_ms", 0)

    atom_attrs = atomize_keys(attrs)
    id = atom_attrs[:id] || attrs["id"]

    case %Monitor{id: id} |> Monitor.changeset(Map.drop(atom_attrs, [:id])) |> Repo.insert() do
      {:ok, monitor} ->
        ensure_monitor_state(monitor.id)
        {:ok, monitor}

      {:error, changeset} ->
        {:error, changeset}
    end
  end

  @spec remove_monitor(String.t()) :: {:ok, Monitor.t()} | {:error, :not_found}
  def remove_monitor(monitor_id) do
    case Repo.get(Monitor, monitor_id) do
      nil -> {:error, :not_found}
      monitor -> Repo.delete(monitor)
    end
  end

  @spec process_check_in(map()) ::
          {:ok, %{check_in: MonitorCheckIn.t(), state: MonitorState.t()}}
          | {:error, :not_found | :invalid_timestamp | {:validation, map()}}
  def process_check_in(params) when is_map(params) do
    with {:ok, monitor_name} <- fetch_required_string(params, "monitor"),
         {:ok, status} <- validate_status(params["status"]),
         {:ok, observed_at} <- parse_timestamp(Map.get(params, "observed_at")),
         %Monitor{} = monitor <- Repo.get_by(Monitor, name: monitor_name) do
      ttl_ms = parse_positive_integer(Map.get(params, "ttl_ms"))
      external_id = optional_string(params["check_in_id"])
      summary = optional_string(params["summary"])
      context = encode_context(Map.get(params, "context"))
      now = DateTime.to_iso8601(observed_at)

      Repo.transaction(fn ->
        state = load_state!(monitor.id)
        old_state = state.state
        effective_interval_ms = effective_interval_ms(monitor, ttl_ms)

        check_in =
          %MonitorCheckIn{id: ID.check_in_id()}
          |> MonitorCheckIn.changeset(%{
            monitor_id: monitor.id,
            external_id: external_id,
            status: status,
            observed_at: now,
            ttl_ms: ttl_ms,
            summary: summary,
            context: context
          })
          |> Repo.insert!()

        attrs =
          transition_attrs_for_check_in(
            state,
            status,
            now,
            deadline_at(now, effective_interval_ms, monitor.grace_ms)
          )

        state = persist_state!(monitor.id, attrs)
        maybe_record_transition!(monitor, state, old_state, now)
        %{check_in: check_in, state: state}
      end)
      |> case do
        {:ok, result} -> {:ok, result}
        {:error, {:validation, errors}} -> {:error, {:validation, errors}}
        {:error, reason} -> {:error, reason}
      end
    else
      nil -> {:error, :not_found}
      {:error, _reason} = error -> error
    end
  end

  @spec evaluate_overdue(keyword()) :: :ok
  def evaluate_overdue(opts \\ []) do
    now = Keyword.get(opts, :at, DateTime.utc_now())
    now_iso = DateTime.to_iso8601(now)

    from(m in Monitor,
      join: s in MonitorState,
      on: s.monitor_id == m.id,
      where: not is_nil(s.deadline_at),
      select: {m, s}
    )
    |> Repo.all()
    |> Enum.each(fn {monitor, state} ->
      maybe_transition_overdue_monitor(monitor, state, now_iso)
    end)

    :ok
  end

  @spec ensure_monitor_state(String.t()) :: :ok
  def ensure_monitor_state(monitor_id) do
    case Repo.get(MonitorState, monitor_id) do
      nil ->
        %MonitorState{monitor_id: monitor_id}
        |> MonitorState.changeset(%{state: "unknown"})
        |> Repo.insert!()

      _ ->
        :ok
    end
  end

  defp maybe_transition_overdue_monitor(monitor, state, now_iso) do
    with {:ok, deadline_dt, _} <- DateTime.from_iso8601(state.deadline_at),
         {:ok, now_dt, _} <- DateTime.from_iso8601(now_iso),
         true <- DateTime.compare(now_dt, deadline_dt) == :gt,
         {:ok, next_state, attrs} <- overdue_transition(monitor, state, now_iso) do
      Repo.transaction(fn ->
        updated =
          persist_state!(
            monitor.id,
            Map.merge(attrs, %{
              state: next_state,
              last_transition_at: now_iso,
              sequence: state.sequence + 1
            })
          )

        payload =
          Timeline.record_monitor_transition!(
            event_name_for(next_state),
            monitor,
            state.state,
            next_state,
            updated,
            updated.sequence,
            now_iso
          )

        Canary.Workers.WebhookDelivery.enqueue_for_event(event_name_for(next_state), payload)
        correlate_incident(monitor)
      end)

      :ok
    else
      _ -> :ok
    end
  rescue
    error ->
      Logger.error(
        "Failed to evaluate overdue monitor #{monitor.id}: #{CorrelationErrorTag.format({:exception, error.__struct__})}"
      )

      :ok
  end

  defp overdue_transition(monitor, state, now_iso) do
    cond do
      state.last_check_in_status == "error" ->
        :noop

      state.state in ["down"] ->
        :noop

      state.state in ["unknown", "up"] ->
        {:ok, "degraded", %{first_missed_at: now_iso}}

      state.state == "degraded" and
          overdue_window_elapsed?(state.first_missed_at, now_iso, monitor.expected_every_ms) ->
        {:ok, "down", %{}}

      true ->
        :noop
    end
  end

  defp overdue_window_elapsed?(nil, _now, _expected_every_ms), do: false

  defp overdue_window_elapsed?(first_missed_at, now_iso, expected_every_ms) do
    with {:ok, first_missed_dt, _} <- DateTime.from_iso8601(first_missed_at),
         {:ok, now_dt, _} <- DateTime.from_iso8601(now_iso) do
      DateTime.diff(now_dt, first_missed_dt, :millisecond) >= expected_every_ms
    else
      _ -> false
    end
  end

  defp load_state!(monitor_id) do
    ensure_monitor_state(monitor_id)
    Repo.get!(MonitorState, monitor_id)
  end

  defp persist_state!(monitor_id, attrs) do
    case Repo.get(MonitorState, monitor_id) do
      nil ->
        %MonitorState{monitor_id: monitor_id}
        |> MonitorState.changeset(attrs)
        |> Repo.insert!()

      state ->
        state
        |> MonitorState.changeset(attrs)
        |> Repo.update!()
    end
  end

  defp transition_attrs_for_check_in(state, status, now, deadline_at) do
    new_state = if(status == "error", do: "down", else: "up")

    base = %{
      state: new_state,
      last_check_in_status: status,
      last_check_in_at: now,
      deadline_at: deadline_at,
      first_missed_at: nil
    }

    base =
      cond do
        status == "error" -> Map.put(base, :last_failure_at, now)
        status in ["alive", "ok"] -> Map.put(base, :last_success_at, now)
        true -> base
      end

    if new_state != state.state do
      base
      |> Map.put(:last_transition_at, now)
      |> Map.put(:sequence, state.sequence + 1)
    else
      base
    end
  end

  defp maybe_record_transition!(monitor, updated_state, old_state, now) do
    if updated_state.state in @transition_states and updated_state.state != old_state do
      payload =
        Timeline.record_monitor_transition!(
          event_name_for(updated_state.state),
          monitor,
          old_state,
          updated_state.state,
          updated_state,
          updated_state.sequence,
          now
        )

      Canary.Workers.WebhookDelivery.enqueue_for_event(
        event_name_for(updated_state.state),
        payload
      )

      correlate_incident(monitor)
    end

    :ok
  end

  defp correlate_incident(monitor) do
    case IncidentCorrelation.safe_correlate(
           :health_transition,
           monitor.id,
           Monitor.service_name(monitor)
         ) do
      {:ok, _incident} ->
        :ok

      {:error, reason} ->
        Logger.error(
          "Failed to correlate incident for monitor #{monitor.id}: #{CorrelationErrorTag.format(reason)}"
        )
    end
  end

  defp event_name_for("up"), do: "health_check.recovered"
  defp event_name_for("degraded"), do: "health_check.degraded"
  defp event_name_for("down"), do: "health_check.down"

  defp deadline_at(observed_at, expected_every_ms, grace_ms) do
    observed_at
    |> DateTime.from_iso8601()
    |> case do
      {:ok, dt, _} ->
        dt
        |> DateTime.add(expected_every_ms + grace_ms, :millisecond)
        |> DateTime.to_iso8601()

      _ ->
        observed_at
    end
  end

  defp effective_interval_ms(%Monitor{mode: "ttl"}, ttl_ms)
       when is_integer(ttl_ms) and ttl_ms > 0,
       do: ttl_ms

  defp effective_interval_ms(%Monitor{expected_every_ms: expected_every_ms}, _ttl_ms),
    do: expected_every_ms

  defp optional_string(value) when is_binary(value) and value != "", do: value
  defp optional_string(_value), do: nil

  defp encode_context(nil), do: nil
  defp encode_context(value) when is_map(value), do: Jason.encode!(value)
  defp encode_context(_value), do: nil

  defp parse_positive_integer(value) when is_integer(value) and value > 0, do: value

  defp parse_positive_integer(value) when is_binary(value) do
    case Integer.parse(value) do
      {int, ""} when int > 0 -> int
      _ -> nil
    end
  end

  defp parse_positive_integer(_value), do: nil

  defp fetch_required_string(params, key) do
    case Map.get(params, key) do
      value when is_binary(value) and value != "" ->
        {:ok, value}

      _ ->
        {:error, {:validation, %{key => ["must be a non-empty string"]}}}
    end
  end

  defp validate_status(status) when status in @check_in_statuses, do: {:ok, status}

  defp validate_status(_status) do
    {:error,
     {:validation,
      %{
        "status" => ["must be one of: alive, in_progress, ok, error"]
      }}}
  end

  defp parse_timestamp(nil), do: {:ok, DateTime.utc_now()}

  defp parse_timestamp(value) when is_binary(value) do
    case DateTime.from_iso8601(value) do
      {:ok, dt, _} -> {:ok, dt}
      _ -> {:error, :invalid_timestamp}
    end
  end

  defp parse_timestamp(_value), do: {:error, :invalid_timestamp}

  defp atomize_keys(map) do
    Map.new(map, fn
      {k, v} when is_binary(k) -> {String.to_existing_atom(k), v}
      {k, v} -> {k, v}
    end)
  rescue
    _ -> map
  end
end

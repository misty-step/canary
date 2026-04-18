defmodule Canary.Query.Health do
  @moduledoc false

  alias Canary.Schemas.{Monitor, MonitorState, Target, TargetCheck, TargetState}

  import Ecto.Query

  @recent_checks_limit 5

  @spec health_targets() :: [map()]
  def health_targets do
    repo = Canary.Repos.read_repo()

    targets_with_state =
      from(t in Target,
        left_join: s in TargetState,
        on: t.id == s.target_id,
        order_by: t.name,
        select: {t, s}
      )
      |> repo.all()

    target_ids = Enum.map(targets_with_state, fn {t, _} -> t.id end)

    checks_by_target = fetch_recent_checks(repo, target_ids)

    Enum.map(targets_with_state, fn {target, state} ->
      recent = Map.get(checks_by_target, target.id, [])

      %{
        id: target.id,
        name: target.name,
        service: Target.service_name(target),
        url: target.url,
        state: (state && state.state) || "unknown",
        consecutive_failures: (state && state.consecutive_failures) || 0,
        last_checked_at: state && state.last_checked_at,
        last_success_at: state && state.last_success_at,
        latency_ms: recent |> List.first() |> then(&(&1 && &1.latency_ms)),
        tls_expires_at: Enum.find_value(recent, & &1.tls_expires_at),
        recent_checks:
          Enum.map(recent, fn c ->
            %{
              checked_at: c.checked_at,
              result: c.result,
              status_code: c.status_code,
              latency_ms: c.latency_ms
            }
          end)
      }
    end)
  end

  @spec health_monitors() :: [map()]
  def health_monitors do
    repo = Canary.Repos.read_repo()

    from(m in Monitor,
      left_join: s in MonitorState,
      on: m.id == s.monitor_id,
      order_by: m.name,
      select: {m, s}
    )
    |> repo.all()
    |> Enum.map(fn {monitor, state} ->
      %{
        id: monitor.id,
        name: monitor.name,
        service: Monitor.service_name(monitor),
        mode: monitor.mode,
        expected_every_ms: monitor.expected_every_ms,
        grace_ms: monitor.grace_ms,
        state: (state && state.state) || "unknown",
        last_check_in_status: state && state.last_check_in_status,
        last_check_in_at: state && state.last_check_in_at,
        last_success_at: state && state.last_success_at,
        last_failure_at: state && state.last_failure_at,
        deadline_at: state && state.deadline_at
      }
    end)
  end

  @spec health_status() :: map()
  def health_status do
    targets = health_targets()
    monitors = health_monitors()
    summary = Canary.Summary.health_status(%{targets: targets, monitors: monitors})
    %{summary: summary, targets: targets, monitors: monitors}
  end

  @spec recent_transitions(String.t(), keyword()) ::
          {:ok, [map()]} | {:error, :invalid_window}
  def recent_transitions(window, opts \\ []) do
    now = Keyword.get(opts, :at, DateTime.utc_now())

    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window, now) do
      target_transitions =
        from(t in Target,
          join: s in TargetState,
          on: t.id == s.target_id,
          where: s.last_transition_at >= ^cutoff,
          select: %{
            id: t.id,
            name: t.name,
            service: t.service,
            state: s.state,
            transitioned_at: s.last_transition_at
          }
        )
        |> Canary.Repos.read_repo().all()
        |> Enum.map(fn transition ->
          %{
            entity_type: "target",
            entity_ref: transition.id,
            name: transition.name,
            service: transition.service || transition.name,
            state: transition.state,
            transitioned_at: transition.transitioned_at
          }
        end)

      monitor_transitions =
        from(m in Monitor,
          join: s in MonitorState,
          on: m.id == s.monitor_id,
          where: s.last_transition_at >= ^cutoff,
          select: %{
            id: m.id,
            name: m.name,
            service: m.service,
            state: s.state,
            transitioned_at: s.last_transition_at
          }
        )
        |> Canary.Repos.read_repo().all()
        |> Enum.map(fn transition ->
          %{
            entity_type: "monitor",
            entity_ref: transition.id,
            name: transition.name,
            service: transition.service || transition.name,
            state: transition.state,
            transitioned_at: transition.transitioned_at
          }
        end)

      transitions =
        (target_transitions ++ monitor_transitions)
        |> Enum.sort_by(
          fn transition ->
            {transition.transitioned_at, transition.entity_type, transition.name}
          end,
          :desc
        )

      {:ok, transitions}
    end
  end

  @spec target_checks(String.t(), String.t()) ::
          {:ok, [%TargetCheck{}]} | {:error, :invalid_window}
  def target_checks(target_id, window) do
    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
      checks =
        from(c in TargetCheck,
          where: c.target_id == ^target_id and c.checked_at >= ^cutoff,
          order_by: [desc: c.checked_at],
          limit: 500
        )
        |> Canary.Repos.read_repo().all()

      {:ok, checks}
    end
  end

  # Batch-fetch top-N recent checks per target using ROW_NUMBER window function.
  # 1 query replaces N individual queries.
  defp fetch_recent_checks(_repo, []), do: %{}

  defp fetch_recent_checks(repo, target_ids) do
    ranked =
      from(c in TargetCheck,
        where: c.target_id in ^target_ids,
        select: %{
          target_id: c.target_id,
          checked_at: c.checked_at,
          result: c.result,
          status_code: c.status_code,
          latency_ms: c.latency_ms,
          tls_expires_at: c.tls_expires_at,
          rn: over(row_number(), :w)
        },
        windows: [w: [partition_by: c.target_id, order_by: [desc: c.checked_at]]]
      )

    from(r in subquery(ranked),
      where: r.rn <= ^@recent_checks_limit,
      order_by: [asc: r.target_id, asc: r.rn]
    )
    |> repo.all()
    |> Enum.group_by(& &1.target_id)
  end
end

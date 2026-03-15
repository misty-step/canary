defmodule Canary.Health.StateMachine do
  @moduledoc """
  Pure state machine for health check targets.

  States: unknown, up, degraded, down, paused, flapping
  Input: :success | :failure
  Output: {new_state, [side_effects]}

  Side effects are data — the caller decides what to do with them.
  """

  @type state :: :unknown | :up | :degraded | :down | :paused | :flapping
  @type event :: :success | :failure
  @type thresholds :: %{
          degraded_after: pos_integer(),
          down_after: pos_integer(),
          up_after: pos_integer()
        }
  @type counters :: %{
          consecutive_failures: non_neg_integer(),
          consecutive_successes: non_neg_integer(),
          transitions: list({state, integer()})
        }
  @type side_effect :: {:transition, state, state} | {:webhook, atom(), map()}

  @flap_window_ms 600_000
  @flap_threshold 4

  @spec transition(state, event, thresholds, counters) ::
          {state, counters, [side_effect]}
  def transition(current_state, event, thresholds, counters) do
    counters = update_counters(counters, event)
    {new_state, counters} = compute_next_state(current_state, event, thresholds, counters)

    {new_state, counters} = detect_flapping(current_state, new_state, counters)

    effects =
      if new_state != current_state do
        [{:transition, current_state, new_state} | webhook_effects(current_state, new_state)]
      else
        []
      end

    {new_state, counters, effects}
  end

  defp update_counters(counters, :success) do
    %{
      counters
      | consecutive_successes: counters.consecutive_successes + 1,
        consecutive_failures: 0
    }
  end

  defp update_counters(counters, :failure) do
    %{
      counters
      | consecutive_failures: counters.consecutive_failures + 1,
        consecutive_successes: 0
    }
  end

  defp compute_next_state(:unknown, :success, _t, c), do: {:up, c}
  defp compute_next_state(:unknown, :failure, _t, c), do: {:degraded, c}

  defp compute_next_state(:up, :failure, t, c) do
    if c.consecutive_failures >= t.degraded_after, do: {:degraded, c}, else: {:up, c}
  end

  defp compute_next_state(:up, :success, _t, c), do: {:up, c}

  defp compute_next_state(:degraded, :success, t, c) do
    if c.consecutive_successes >= t.up_after, do: {:up, c}, else: {:degraded, c}
  end

  defp compute_next_state(:degraded, :failure, t, c) do
    if c.consecutive_failures >= t.down_after, do: {:down, c}, else: {:degraded, c}
  end

  defp compute_next_state(:down, :success, t, c) do
    if c.consecutive_successes >= t.up_after, do: {:up, c}, else: {:down, c}
  end

  defp compute_next_state(:down, :failure, _t, c), do: {:down, c}

  defp compute_next_state(:paused, _event, _t, c), do: {:paused, c}

  defp compute_next_state(:flapping, :success, t, c) do
    if c.consecutive_successes >= t.up_after, do: {:up, c}, else: {:flapping, c}
  end

  defp compute_next_state(:flapping, :failure, t, c) do
    if c.consecutive_failures >= t.down_after, do: {:down, c}, else: {:flapping, c}
  end

  defp detect_flapping(old_state, new_state, counters) when old_state != new_state do
    now = System.monotonic_time(:millisecond)
    transitions = [{new_state, now} | counters.transitions || []]

    recent =
      Enum.filter(transitions, fn {_s, ts} -> now - ts < @flap_window_ms end)

    if length(recent) >= @flap_threshold and new_state not in [:paused, :flapping] do
      {:flapping, %{counters | transitions: recent}}
    else
      {new_state, %{counters | transitions: recent}}
    end
  end

  defp detect_flapping(_old, new_state, counters), do: {new_state, counters}

  defp webhook_effects(_from, :up), do: [{:webhook, :health_check_recovered, %{}}]
  defp webhook_effects(_from, :degraded), do: [{:webhook, :health_check_degraded, %{}}]
  defp webhook_effects(_from, :down), do: [{:webhook, :health_check_down, %{}}]
  defp webhook_effects(_from, _to), do: []

  def initial_counters do
    %{consecutive_failures: 0, consecutive_successes: 0, transitions: []}
  end
end

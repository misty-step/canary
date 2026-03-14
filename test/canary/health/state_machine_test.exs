defmodule Canary.Health.StateMachineTest do
  use ExUnit.Case, async: true

  alias Canary.Health.StateMachine

  @defaults %{degraded_after: 1, down_after: 3, up_after: 1}

  defp initial, do: StateMachine.initial_counters()

  describe "unknown state" do
    test "transitions to up on first success" do
      {state, _c, effects} = StateMachine.transition(:unknown, :success, @defaults, initial())
      assert state == :up
      assert {:transition, :unknown, :up} in effects
    end

    test "transitions to degraded on first failure" do
      {state, _c, effects} = StateMachine.transition(:unknown, :failure, @defaults, initial())
      assert state == :degraded
      assert {:transition, :unknown, :degraded} in effects
    end
  end

  describe "up state" do
    test "stays up on success" do
      counters = %{initial() | consecutive_successes: 1}
      {state, _c, effects} = StateMachine.transition(:up, :success, @defaults, counters)
      assert state == :up
      assert effects == []
    end

    test "transitions to degraded after degraded_after failures" do
      counters = %{initial() | consecutive_failures: 1}
      {state, _c, effects} = StateMachine.transition(:up, :failure, @defaults, counters)
      assert state == :degraded
      assert {:transition, :up, :degraded} in effects
    end

    test "stays up if failures < degraded_after" do
      thresholds = %{@defaults | degraded_after: 3}
      counters = %{initial() | consecutive_failures: 1}
      {state, _c, _effects} = StateMachine.transition(:up, :failure, thresholds, counters)
      assert state == :up
    end
  end

  describe "degraded state" do
    test "recovers to up after up_after successes" do
      counters = %{initial() | consecutive_successes: 1}
      {state, _c, effects} = StateMachine.transition(:degraded, :success, @defaults, counters)
      assert state == :up
      assert {:transition, :degraded, :up} in effects
    end

    test "transitions to down after down_after consecutive failures" do
      counters = %{initial() | consecutive_failures: 3}
      {state, _c, effects} = StateMachine.transition(:degraded, :failure, @defaults, counters)
      assert state == :down
      assert {:transition, :degraded, :down} in effects
    end

    test "stays degraded if failures < down_after" do
      counters = %{initial() | consecutive_failures: 1}
      {state, _c, _effects} = StateMachine.transition(:degraded, :failure, @defaults, counters)
      assert state == :degraded
    end
  end

  describe "down state" do
    test "recovers to up after up_after successes" do
      counters = %{initial() | consecutive_successes: 1}
      {state, _c, effects} = StateMachine.transition(:down, :success, @defaults, counters)
      assert state == :up
      assert {:transition, :down, :up} in effects
    end

    test "stays down on failure" do
      counters = %{initial() | consecutive_failures: 5}
      {state, _c, effects} = StateMachine.transition(:down, :failure, @defaults, counters)
      assert state == :down
      assert effects == []
    end
  end

  describe "webhook effects" do
    test "recovered webhook on transition to up" do
      counters = %{initial() | consecutive_successes: 1}
      {_state, _c, effects} = StateMachine.transition(:down, :success, @defaults, counters)
      assert Enum.any?(effects, fn {:webhook, :health_check_recovered, _} -> true; _ -> false end)
    end

    test "degraded webhook on transition to degraded" do
      counters = %{initial() | consecutive_failures: 1}
      {_state, _c, effects} = StateMachine.transition(:up, :failure, @defaults, counters)
      assert Enum.any?(effects, fn {:webhook, :health_check_degraded, _} -> true; _ -> false end)
    end

    test "down webhook on transition to down" do
      counters = %{initial() | consecutive_failures: 3}
      {_state, _c, effects} = StateMachine.transition(:degraded, :failure, @defaults, counters)
      assert Enum.any?(effects, fn {:webhook, :health_check_down, _} -> true; _ -> false end)
    end
  end

  describe "counter management" do
    test "success resets failure counter" do
      counters = %{initial() | consecutive_failures: 5}
      {_state, c, _effects} = StateMachine.transition(:down, :success, @defaults, counters)
      assert c.consecutive_failures == 0
      assert c.consecutive_successes == 1
    end

    test "failure resets success counter" do
      counters = %{initial() | consecutive_successes: 3}
      {_state, c, _effects} = StateMachine.transition(:up, :failure, @defaults, counters)
      assert c.consecutive_successes == 0
      assert c.consecutive_failures == 1
    end
  end
end

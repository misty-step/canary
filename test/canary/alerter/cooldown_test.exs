defmodule Canary.Alerter.CooldownTest do
  use ExUnit.Case, async: false

  alias Canary.Alerter.Cooldown

  @table :canary_cooldowns

  setup do
    :ets.delete_all_objects(@table)
    :ok
  end

  test "in_cooldown?/1 returns false for unknown key" do
    refute Cooldown.in_cooldown?("unknown:key")
  end

  test "in_cooldown?/1 returns true after mark/1" do
    Cooldown.mark("whk-1:error.new_class")
    assert Cooldown.in_cooldown?("whk-1:error.new_class")
  end

  test "in_cooldown?/1 returns false after cooldown expires" do
    # Insert with timestamp far in the past (> 5 min ago)
    old_time = System.monotonic_time(:millisecond) - 400_000
    :ets.insert(@table, {"whk-expired:event", old_time})

    refute Cooldown.in_cooldown?("whk-expired:event")
  end

  test "mark/1 updates timestamp for existing key" do
    Cooldown.mark("whk-update:event")
    [{_, ts1}] = :ets.lookup(@table, "whk-update:event")

    Process.sleep(1)
    Cooldown.mark("whk-update:event")
    [{_, ts2}] = :ets.lookup(@table, "whk-update:event")

    assert ts2 > ts1
  end

  test "different keys are independent" do
    Cooldown.mark("whk-a:event1")

    assert Cooldown.in_cooldown?("whk-a:event1")
    refute Cooldown.in_cooldown?("whk-a:event2")
    refute Cooldown.in_cooldown?("whk-b:event1")
  end
end

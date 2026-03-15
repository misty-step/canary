defmodule Canary.Alerter.CircuitBreakerTest do
  use ExUnit.Case, async: false

  alias Canary.Alerter.CircuitBreaker

  @table :canary_circuit_breakers

  setup do
    :ets.delete_all_objects(@table)
    :ok
  end

  test "open?/1 returns false with no failures" do
    refute CircuitBreaker.open?("whk-fresh")
  end

  test "open?/1 returns false below threshold" do
    for _ <- 1..9, do: CircuitBreaker.record_failure("whk-nine")
    refute CircuitBreaker.open?("whk-nine")
  end

  test "open?/1 returns true after 10 failures" do
    for _ <- 1..10, do: CircuitBreaker.record_failure("whk-ten")
    assert CircuitBreaker.open?("whk-ten")
  end

  test "record_success/1 resets the breaker" do
    for _ <- 1..10, do: CircuitBreaker.record_failure("whk-reset")
    assert CircuitBreaker.open?("whk-reset")

    CircuitBreaker.record_success("whk-reset")
    refute CircuitBreaker.open?("whk-reset")
  end

  test "should_probe?/1 returns false immediately after opening" do
    for _ <- 1..10, do: CircuitBreaker.record_failure("whk-probe")
    assert CircuitBreaker.open?("whk-probe")
    refute CircuitBreaker.should_probe?("whk-probe")
  end

  test "should_probe?/1 returns true after probe interval" do
    # Manually insert with a suspended_at far in the past
    old_time = System.monotonic_time(:millisecond) - 400_000
    :ets.insert(@table, {"whk-old", 10, old_time})

    assert CircuitBreaker.open?("whk-old")
    assert CircuitBreaker.should_probe?("whk-old")
  end

  test "should_probe?/1 returns false for non-open breaker" do
    refute CircuitBreaker.should_probe?("whk-nonexistent")
  end

  test "record_failure/1 increments count" do
    CircuitBreaker.record_failure("whk-inc")
    [{_, count, _}] = :ets.lookup(@table, "whk-inc")
    assert count == 1

    CircuitBreaker.record_failure("whk-inc")
    [{_, count, _}] = :ets.lookup(@table, "whk-inc")
    assert count == 2
  end
end

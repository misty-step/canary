defmodule Canary.Webhooks.EventTypesTest do
  use ExUnit.Case, async: true

  alias Canary.Webhooks.EventTypes

  @business_events ~w(
    health_check.degraded health_check.down health_check.recovered
    health_check.tls_expiring error.new_class error.regression
    incident.opened incident.updated incident.resolved
    annotation.added
  )

  describe "all/0" do
    test "includes canary.ping" do
      assert "canary.ping" in EventTypes.all()
    end

    test "includes all business events" do
      for event <- @business_events do
        assert event in EventTypes.all(), "expected #{event} in all()"
      end
    end
  end

  describe "business/0" do
    test "returns exactly the 10 business events" do
      assert Enum.sort(EventTypes.business()) == Enum.sort(@business_events)
    end

    test "does not include canary.ping" do
      refute "canary.ping" in EventTypes.business()
    end
  end

  describe "valid?/1" do
    test "canary.ping is valid" do
      assert EventTypes.valid?("canary.ping")
    end

    test "business events are valid" do
      for event <- @business_events do
        assert EventTypes.valid?(event), "expected #{event} to be valid"
      end
    end

    test "unknown events are invalid" do
      refute EventTypes.valid?("bogus.event")
    end
  end

  describe "diagnostic?/1" do
    test "canary.ping is diagnostic" do
      assert EventTypes.diagnostic?("canary.ping")
    end

    test "business events are not diagnostic" do
      refute EventTypes.diagnostic?("error.new_class")
    end
  end
end

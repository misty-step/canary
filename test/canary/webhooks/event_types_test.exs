defmodule Canary.Webhooks.EventTypesTest do
  use ExUnit.Case, async: true

  alias Canary.Webhooks.EventTypes

  describe "valid?/1" do
    test "canary.ping is valid" do
      assert EventTypes.valid?("canary.ping")
    end

    test "business events are valid" do
      assert EventTypes.valid?("error.new_class")
      assert EventTypes.valid?("incident.opened")
      assert EventTypes.valid?("health_check.down")
    end

    test "unknown events are invalid" do
      refute EventTypes.valid?("bogus.event")
    end
  end

  describe "timeline/0" do
    test "excludes diagnostic events" do
      refute "canary.ping" in EventTypes.timeline()
    end

    test "includes business events" do
      assert "error.new_class" in EventTypes.timeline()
      assert "incident.opened" in EventTypes.timeline()
    end
  end
end

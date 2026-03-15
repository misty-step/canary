defmodule CanaryTriage.SynthesizerTest do
  use ExUnit.Case, async: true

  alias CanaryTriage.Synthesizer

  defp health_payload(state, prev \\ "healthy") do
    %{
      "event" => "health_check.#{state}",
      "state" => state,
      "previous_state" => prev,
      "timestamp" => "2026-03-15T10:00:00Z",
      "consecutive_failures" => 3,
      "last_success_at" => "2026-03-15T09:55:00Z",
      "target" => %{"name" => "canary-triage", "url" => "https://canary-triage.fly.dev/healthz"},
      "last_check" => %{"result" => "timeout", "status_code" => 0, "latency_ms" => 5000}
    }
  end

  describe "build_health_check_issue/1" do
    test "degraded: correct title, labels, priority" do
      {:ok, issue} = Synthesizer.build_health_check_issue(health_payload("degraded"))

      assert issue["title"] == "Health Check Degraded: canary-triage"
      assert "health-check" in issue["labels"]
      assert "high-priority" in issue["labels"]
      assert issue["priority"] == "high"
    end

    test "down: critical priority" do
      {:ok, issue} = Synthesizer.build_health_check_issue(health_payload("down"))

      assert issue["title"] == "Health Check Down: canary-triage"
      assert "critical" in issue["labels"]
      assert issue["priority"] == "critical"
    end

    test "body includes service details and investigation steps" do
      {:ok, issue} = Synthesizer.build_health_check_issue(health_payload("degraded"))

      assert issue["body"] =~ "canary-triage"
      assert issue["body"] =~ "canary-triage.fly.dev"
      assert issue["body"] =~ "flyctl status"
      assert issue["body"] =~ "Consecutive Failures"
    end
  end

  describe "build_health_check_comment/1" do
    test "includes state and check details" do
      comment = Synthesizer.build_health_check_comment(health_payload("degraded"))

      assert comment =~ "Still Degraded" || comment =~ "Degraded"
      assert comment =~ "5000"
      assert comment =~ "3"
    end
  end

  describe "build_recovery_comment/1" do
    test "includes recovery details" do
      payload = health_payload("healthy", "degraded")
      comment = Synthesizer.build_recovery_comment(payload)

      assert comment =~ "Recovered"
      assert comment =~ "canary-triage"
      assert comment =~ "healthy"
      assert comment =~ "degraded"
    end
  end
end

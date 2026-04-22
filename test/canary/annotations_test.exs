defmodule Canary.AnnotationsTest do
  use Canary.DataCase

  import Canary.Fixtures

  alias Canary.Annotations

  setup do
    clean_status_tables()
    :ok
  end

  describe "create_for_incident/2" do
    test "creates annotation for existing incident" do
      incident = create_incident("test-svc")

      assert {:ok, ann} =
               Annotations.create_for_incident(incident.id, %{
                 "agent" => "triage-bot",
                 "action" => "acknowledged",
                 "metadata" => %{"reason" => "auto-triage"}
               })

      assert String.starts_with?(ann.id, "ANN-")
      assert ann.incident_id == incident.id
      assert ann.agent == "triage-bot"
      assert ann.action == "acknowledged"
      assert ann.metadata == ~s({"reason":"auto-triage"})
      assert ann.created_at != nil
    end

    test "returns :not_found for nonexistent incident" do
      assert {:error, :not_found} =
               Annotations.create_for_incident("INC-nonexistent", %{
                 "agent" => "bot",
                 "action" => "ack"
               })
    end
  end

  describe "list_for_incident/1" do
    test "returns :not_found for nonexistent incident" do
      assert {:error, :not_found} = Annotations.list_for_incident("INC-nonexistent")
    end

    test "returns annotations for existing incident" do
      incident = create_incident("test-svc")

      {:ok, _} =
        Annotations.create_for_incident(incident.id, %{
          "agent" => "bot-a",
          "action" => "acknowledged"
        })

      {:ok, _} =
        Annotations.create_for_incident(incident.id, %{
          "agent" => "bot-b",
          "action" => "triaged"
        })

      {:ok, annotations} = Annotations.list_for_incident(incident.id)
      assert length(annotations) == 2
      agents = Enum.map(annotations, & &1.agent)
      assert MapSet.new(agents) == MapSet.new(["bot-a", "bot-b"])
    end
  end

  describe "create_for_group/2" do
    test "creates annotation for existing error group" do
      group = create_error_group("test-svc", "RuntimeError", 5)

      assert {:ok, ann} =
               Annotations.create_for_group(group.group_hash, %{
                 "agent" => "fix-bot",
                 "action" => "fix_deployed"
               })

      assert String.starts_with?(ann.id, "ANN-")
      assert ann.group_hash == group.group_hash
      assert ann.agent == "fix-bot"
      assert ann.action == "fix_deployed"
    end

    test "returns :not_found for nonexistent group" do
      assert {:error, :not_found} =
               Annotations.create_for_group("nonexistent-hash", %{
                 "agent" => "bot",
                 "action" => "ack"
               })
    end
  end

  describe "list_for_group/1" do
    test "returns :not_found for nonexistent group" do
      assert {:error, :not_found} = Annotations.list_for_group("nonexistent-hash")
    end

    test "returns annotations for a group" do
      group = create_error_group("test-svc", "RuntimeError", 5)

      {:ok, _} =
        Annotations.create_for_group(group.group_hash, %{
          "agent" => "bot-a",
          "action" => "acknowledged"
        })

      {:ok, annotations} = Annotations.list_for_group(group.group_hash)
      assert length(annotations) == 1
      assert hd(annotations).group_hash == group.group_hash
    end
  end

  describe "create/1" do
    test "creates annotation on incident via unified API" do
      incident = create_incident("test-svc")

      assert {:ok, ann} =
               Annotations.create(%{
                 "subject_type" => "incident",
                 "subject_id" => incident.id,
                 "agent" => "bot",
                 "action" => "acknowledged"
               })

      assert ann.subject_type == "incident"
      assert ann.subject_id == incident.id
      assert ann.incident_id == incident.id
      assert ann.group_hash == nil
    end

    test "creates annotation on error_group via unified API" do
      group = create_error_group("test-svc", "RuntimeError", 5)

      assert {:ok, ann} =
               Annotations.create(%{
                 "subject_type" => "error_group",
                 "subject_id" => group.group_hash,
                 "agent" => "fix-bot",
                 "action" => "fix_deployed"
               })

      assert ann.subject_type == "error_group"
      assert ann.subject_id == group.group_hash
      assert ann.group_hash == group.group_hash
      assert ann.incident_id == nil
    end

    test "creates annotation on target via unified API" do
      create_target_with_state("api-svc", "up")

      assert {:ok, ann} =
               Annotations.create(%{
                 "subject_type" => "target",
                 "subject_id" => "TGT-api-svc",
                 "agent" => "triage-bot",
                 "action" => "paged"
               })

      assert ann.subject_type == "target"
      assert ann.subject_id == "TGT-api-svc"
      assert ann.incident_id == nil
      assert ann.group_hash == nil
    end

    test "creates annotation on monitor via unified API" do
      create_monitor_with_state("cron-daily", "alive")

      assert {:ok, ann} =
               Annotations.create(%{
                 "subject_type" => "monitor",
                 "subject_id" => "MON-cron-daily",
                 "agent" => "triage-bot",
                 "action" => "silenced"
               })

      assert ann.subject_type == "monitor"
      assert ann.subject_id == "MON-cron-daily"
    end

    test "returns :not_found when subject does not exist" do
      assert {:error, :not_found} =
               Annotations.create(%{
                 "subject_type" => "target",
                 "subject_id" => "TGT-nope",
                 "agent" => "bot",
                 "action" => "ack"
               })
    end

    test "returns :invalid_subject_type for unknown type" do
      assert {:error, :invalid_subject_type} =
               Annotations.create(%{
                 "subject_type" => "spaceship",
                 "subject_id" => "X-1",
                 "agent" => "bot",
                 "action" => "ack"
               })
    end

    test "returns :invalid_subject when subject_type/id missing" do
      assert {:error, :invalid_subject} =
               Annotations.create(%{"agent" => "bot", "action" => "ack"})
    end
  end

  describe "list/2" do
    test "lists annotations on any subject type" do
      create_target_with_state("api-svc", "up")

      {:ok, _} =
        Annotations.create(%{
          "subject_type" => "target",
          "subject_id" => "TGT-api-svc",
          "agent" => "bot-a",
          "action" => "paged"
        })

      {:ok, _} =
        Annotations.create(%{
          "subject_type" => "target",
          "subject_id" => "TGT-api-svc",
          "agent" => "bot-b",
          "action" => "silenced"
        })

      assert {:ok, rows} = Annotations.list("target", "TGT-api-svc")
      assert length(rows) == 2
      agents = Enum.map(rows, & &1.agent)
      assert "bot-a" in agents and "bot-b" in agents
    end

    test "returns :not_found when subject does not exist" do
      assert {:error, :not_found} = Annotations.list("target", "TGT-nope")
    end

    test "returns :invalid_subject_type for unknown type" do
      assert {:error, :invalid_subject_type} = Annotations.list("spaceship", "X-1")
    end
  end

  describe "count_by_subject/1" do
    test "returns zero-length map for empty keys" do
      assert Annotations.count_by_subject([]) == %{}
    end

    test "counts annotations grouped by (subject_type, subject_id)" do
      group_a = create_error_group("svc-a", "RuntimeError", 1)
      group_b = create_error_group("svc-b", "ArgumentError", 1)
      create_target_with_state("tgt-1", "up")

      for agent <- ["a", "b", "c"] do
        {:ok, _} =
          Annotations.create(%{
            "subject_type" => "error_group",
            "subject_id" => group_a.group_hash,
            "agent" => agent,
            "action" => "ack"
          })
      end

      {:ok, _} =
        Annotations.create(%{
          "subject_type" => "target",
          "subject_id" => "TGT-tgt-1",
          "agent" => "bot",
          "action" => "paged"
        })

      counts =
        Annotations.count_by_subject([
          {"error_group", group_a.group_hash},
          {"error_group", group_b.group_hash},
          {"target", "TGT-tgt-1"},
          {"monitor", "MON-never-existed"}
        ])

      assert counts[{"error_group", group_a.group_hash}] == 3
      assert counts[{"target", "TGT-tgt-1"}] == 1
      refute Map.has_key?(counts, {"error_group", group_b.group_hash})
      refute Map.has_key?(counts, {"monitor", "MON-never-existed"})
    end
  end

  describe "format/1" do
    test "returns presentation map with decoded metadata" do
      incident = create_incident("test-svc")

      {:ok, ann} =
        Annotations.create_for_incident(incident.id, %{
          "agent" => "triage-bot",
          "action" => "acknowledged",
          "metadata" => %{"reason" => "auto-triage"}
        })

      formatted = Annotations.format(ann)
      assert formatted.id == ann.id
      assert formatted.incident_id == incident.id
      assert formatted.group_hash == nil
      assert formatted.agent == "triage-bot"
      assert formatted.action == "acknowledged"
      assert formatted.metadata == %{"reason" => "auto-triage"}
      assert formatted.created_at != nil
    end

    test "handles nil metadata" do
      incident = create_incident("test-svc")

      {:ok, ann} =
        Annotations.create_for_incident(incident.id, %{
          "agent" => "bot",
          "action" => "ack"
        })

      formatted = Annotations.format(ann)
      assert formatted.metadata == nil
    end

    test "drops non-map non-string metadata to nil" do
      incident = create_incident("test-svc")

      {:ok, ann} =
        Annotations.create_for_incident(incident.id, %{
          "agent" => "bot",
          "action" => "ack",
          "metadata" => [1, 2, 3]
        })

      assert ann.metadata == nil
    end

    test "handles non-JSON string metadata gracefully" do
      incident = create_incident("test-svc")

      {:ok, ann} =
        Annotations.create_for_incident(incident.id, %{
          "agent" => "bot",
          "action" => "ack",
          "metadata" => "plain string"
        })

      formatted = Annotations.format(ann)
      assert formatted.metadata == "plain string"
    end
  end
end

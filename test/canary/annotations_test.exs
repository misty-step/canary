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

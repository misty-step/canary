defmodule Canary.Workers.RetentionPruneTest do
  use Canary.DataCase

  alias Canary.Workers.RetentionPrune
  alias Canary.Schemas.{Error, ServiceEvent, Target, TargetCheck}

  defp insert_error(days_ago) do
    id = Canary.ID.error_id()
    created = DateTime.utc_now() |> DateTime.add(-days_ago, :day) |> DateTime.to_iso8601()

    %Error{id: id}
    |> Error.changeset(%{
      service: "test-svc",
      error_class: "TestError",
      message: "test error #{id}",
      group_hash: "grp-#{id}",
      created_at: created
    })
    |> Repo.insert!()
  end

  defp insert_target do
    id = Canary.ID.target_id()
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    %Target{id: id}
    |> Target.changeset(%{
      url: "https://example.com",
      name: "test",
      service: "test",
      created_at: now
    })
    |> Repo.insert!()
  end

  defp insert_event(days_ago) do
    id = Canary.ID.event_id()
    created = DateTime.utc_now() |> DateTime.add(-days_ago, :day) |> DateTime.to_iso8601()

    %ServiceEvent{id: id}
    |> ServiceEvent.changeset(%{
      service: "test",
      event: "error.new_class",
      entity_type: "error_group",
      entity_ref: "group-#{id}",
      severity: "error",
      summary: "test event #{id}",
      payload: Jason.encode!(%{"event" => "error.new_class"}),
      created_at: created
    })
    |> Repo.insert!()
  end

  defp insert_check(target_id, days_ago) do
    checked = DateTime.utc_now() |> DateTime.add(-days_ago, :day) |> DateTime.to_iso8601()

    %TargetCheck{}
    |> TargetCheck.changeset(%{target_id: target_id, checked_at: checked, result: "success"})
    |> Repo.insert!()
  end

  describe "perform/1" do
    test "deletes errors older than retention_days" do
      old = insert_error(31)
      recent = insert_error(1)

      assert :ok = RetentionPrune.perform(%Oban.Job{})

      refute Repo.get(Error, old.id)
      assert Repo.get(Error, recent.id)
    end

    test "deletes checks older than check_retention_days" do
      target = insert_target()
      old = insert_check(target.id, 8)
      recent = insert_check(target.id, 1)

      assert :ok = RetentionPrune.perform(%Oban.Job{})

      refute Repo.get(TargetCheck, old.id)
      assert Repo.get(TargetCheck, recent.id)
    end

    test "deletes service events older than retention_days" do
      old = insert_event(31)
      recent = insert_event(1)

      assert :ok = RetentionPrune.perform(%Oban.Job{})

      refute Repo.get(ServiceEvent, old.id)
      assert Repo.get(ServiceEvent, recent.id)
    end

    test "handles empty tables" do
      assert :ok = RetentionPrune.perform(%Oban.Job{})
    end
  end
end

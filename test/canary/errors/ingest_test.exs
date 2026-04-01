defmodule Canary.Errors.IngestTest do
  use Canary.DataCase

  alias Canary.Errors.Ingest
  alias Canary.Schemas.{Error, ErrorGroup, Incident, ServiceEvent}

  @valid_attrs %{
    "service" => "cadence",
    "error_class" => "RuntimeError",
    "message" => "something went wrong"
  }

  setup do
    Canary.Fixtures.clean_status_tables()
    :ets.delete_all_objects(:canary_dedup_cache)
    :ok
  end

  describe "ingest/1" do
    test "creates error and group for new error" do
      {:ok, result} = Ingest.ingest(@valid_attrs)

      assert String.starts_with?(result.id, "ERR-")
      assert is_binary(result.group_hash)
      assert result.is_new_class == true

      assert Repo.get(Error, result.id)
      assert Repo.get(ErrorGroup, result.group_hash)
    end

    test "records a timeline event for a new error class" do
      {:ok, result} = Ingest.ingest(@valid_attrs)

      event =
        Repo.one!(
          from(e in ServiceEvent,
            where: e.event == "error.new_class" and e.entity_ref == ^result.group_hash
          )
        )

      payload = Jason.decode!(event.payload)

      assert event.service == "cadence"
      assert payload["event"] == "error.new_class"
      assert payload["error"]["service"] == "cadence"
      assert payload["error"]["group_hash"] == result.group_hash
    end

    test "increments group count on duplicate" do
      {:ok, r1} = Ingest.ingest(@valid_attrs)
      {:ok, r2} = Ingest.ingest(@valid_attrs)

      assert r1.group_hash == r2.group_hash
      assert r2.is_new_class == false

      group = Repo.get(ErrorGroup, r1.group_hash)
      assert group.total_count == 2
    end

    test "uses fingerprint for grouping when provided" do
      attrs = Map.put(@valid_attrs, "fingerprint", ["custom-group"])
      {:ok, r1} = Ingest.ingest(attrs)

      attrs2 =
        Map.merge(@valid_attrs, %{
          "message" => "totally different",
          "fingerprint" => ["custom-group"]
        })

      {:ok, r2} = Ingest.ingest(attrs2)

      assert r1.group_hash == r2.group_hash
    end

    test "rejects missing required fields" do
      {:error, :validation_error, errors} = Ingest.ingest(%{"service" => "svc"})
      field_names = Enum.map(errors, fn {name, _} -> name end)
      assert "error_class" in field_names
      assert "message" in field_names
    end

    test "stores severity and environment defaults" do
      {:ok, result} = Ingest.ingest(@valid_attrs)
      error = Repo.get(Error, result.id)

      assert error.severity == "error"
      assert error.environment == "production"
    end

    test "stores classification on the error record" do
      attrs = Map.put(@valid_attrs, "error_class", "DBConnection.ConnectionError")

      {:ok, result} = Ingest.ingest(attrs)
      error = Repo.get(Error, result.id)

      assert error.classification_category == "infrastructure"
      assert error.classification_persistence == "transient"
      assert error.classification_component == "database"
    end

    test "accepts custom severity" do
      attrs = Map.put(@valid_attrs, "severity", "warning")
      {:ok, result} = Ingest.ingest(attrs)
      error = Repo.get(Error, result.id)

      assert error.severity == "warning"
    end

    test "rejects non-string fingerprint elements" do
      attrs = Map.put(@valid_attrs, "fingerprint", ["ok", 123])
      {:error, :validation_error, errors} = Ingest.ingest(attrs)
      assert errors == %{"fingerprint" => ["elements must be strings"]}
    end

    test "rejects non-list fingerprint" do
      attrs = Map.put(@valid_attrs, "fingerprint", 123)
      {:error, :validation_error, errors} = Ingest.ingest(attrs)
      assert errors == %{"fingerprint" => ["must be a list of strings"]}
    end

    test "broadcasts new error via PubSub after commit" do
      Phoenix.PubSub.subscribe(Canary.PubSub, "errors:new")

      {:ok, result} = Ingest.ingest(@valid_attrs)

      assert_receive {:new_error, error}
      assert error.id == result.id
      assert error.service == "cadence"
    end

    test "attaches a new error group to the existing service incident" do
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      Repo.insert!(%Canary.Schemas.Target{
        id: "TGT-cadence",
        name: "cadence",
        service: "cadence",
        url: "https://cadence.example.com",
        created_at: now
      })

      Repo.insert!(%Canary.Schemas.TargetState{
        target_id: "TGT-cadence",
        state: "degraded",
        consecutive_failures: 1,
        last_checked_at: now
      })

      {:ok, _incident} = Canary.Incidents.correlate(:health_transition, "TGT-cadence", "cadence")
      {:ok, _result} = Ingest.ingest(@valid_attrs)

      incident =
        Repo.one!(
          from(i in Incident,
            preload: [:signals],
            where: i.service == "cadence" and i.state == "investigating"
          )
        )

      assert length(incident.signals) == 2
      assert Enum.any?(incident.signals, &(&1.signal_type == "health_transition"))
      assert Enum.any?(incident.signals, &(&1.signal_type == "error_group"))
    end
  end
end

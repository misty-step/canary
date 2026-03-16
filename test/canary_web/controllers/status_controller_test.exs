defmodule CanaryWeb.StatusControllerTest do
  use CanaryWeb.ConnCase

  setup %{conn: conn} do
    # Clear pre-existing data (Health.Manager boot) within sandbox
    Canary.Repo.delete_all(Canary.Schemas.TargetState)
    Canary.Repo.delete_all(Canary.Schemas.TargetCheck)
    Canary.Repo.delete_all(Canary.Schemas.Target)
    Canary.Repo.delete_all(Canary.Schemas.ErrorGroup)

    {raw_key, _key} = create_api_key()
    conn = authenticate(conn, raw_key)
    {:ok, conn: conn}
  end

  describe "GET /api/v1/status" do
    test "all healthy, no errors", %{conn: conn} do
      # Create 3 healthy targets
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      for name <- ["alpha", "bravo", "charlie"] do
        id = "TGT-#{name}"

        Canary.Repo.insert!(%Canary.Schemas.Target{
          id: id,
          name: name,
          url: "https://#{name}.example.com/healthz",
          created_at: now
        })

        Canary.Repo.insert!(%Canary.Schemas.TargetState{
          target_id: id,
          state: "up",
          consecutive_failures: 0,
          last_checked_at: now,
          last_success_at: now
        })
      end

      conn = get(conn, "/api/v1/status")
      body = json_response(conn, 200)

      assert body["overall"] == "healthy"
      assert length(body["targets"]) == 3
      assert body["error_summary"] == []
      assert is_binary(body["summary"])
    end

    test "target down with errors", %{conn: conn} do
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      Canary.Repo.insert!(%Canary.Schemas.Target{
        id: "TGT-volume",
        name: "volume",
        url: "https://volume.example.com/healthz",
        created_at: now
      })

      Canary.Repo.insert!(%Canary.Schemas.TargetState{
        target_id: "TGT-volume",
        state: "down",
        consecutive_failures: 5,
        last_checked_at: now
      })

      group_hash =
        :crypto.hash(:sha256, "volume:ConnectionError") |> Base.encode16(case: :lower)

      for i <- 1..12 do
        Canary.Repo.insert!(
          %Canary.Schemas.Error{
            id: "ERR-vol-#{i}",
            service: "volume",
            error_class: "ConnectionError",
            message: "connection refused",
            message_template: "connection refused",
            severity: "error",
            environment: "production",
            group_hash: group_hash,
            created_at: now
          },
          on_conflict: :nothing
        )
      end

      Canary.Repo.insert!(%Canary.Schemas.ErrorGroup{
        group_hash: group_hash,
        service: "volume",
        error_class: "ConnectionError",
        severity: "error",
        first_seen_at: now,
        last_seen_at: now,
        total_count: 12,
        last_error_id: "ERR-vol-12"
      })

      conn = get(conn, "/api/v1/status")
      body = json_response(conn, 200)

      assert body["overall"] == "unhealthy"
      assert [error_entry] = body["error_summary"]
      assert error_entry["service"] == "volume"
      assert error_entry["total_count"] == 12
    end

    test "no targets and no errors", %{conn: conn} do
      conn = get(conn, "/api/v1/status")
      body = json_response(conn, 200)

      assert body["overall"] == "empty"
      assert body["targets"] == []
      assert body["error_summary"] == []
      assert body["summary"] =~ "No services configured"
    end

    test "401 without auth", %{conn: conn} do
      conn =
        conn
        |> delete_req_header("authorization")
        |> get("/api/v1/status")

      assert json_response(conn, 401)["code"] == "invalid_api_key"
    end
  end
end

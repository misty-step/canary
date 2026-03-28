defmodule CanaryWeb.TimelineControllerTest do
  use CanaryWeb.ConnCase

  import Canary.Fixtures

  alias Canary.ID
  alias Canary.Schemas.ServiceEvent

  setup %{conn: conn} do
    clean_status_tables()
    {raw_key, _key} = create_api_key()
    conn = authenticate(conn, raw_key)
    {:ok, conn: conn}
  end

  describe "GET /api/v1/timeline" do
    test "returns timeline events filtered by service with cursor pagination", %{conn: conn} do
      insert_event!(%{service: "alpha", event: "error.new_class", created_at: ts(-10)})
      insert_event!(%{service: "alpha", event: "incident.opened", created_at: ts(-20)})
      insert_event!(%{service: "beta", event: "error.new_class", created_at: ts(-30)})

      first_conn = get(conn, "/api/v1/timeline?service=alpha&limit=1&window=24h")
      first = json_response(first_conn, 200)

      assert first["service"] == "alpha"
      assert length(first["events"]) == 1
      assert hd(first["events"])["event"] == "error.new_class"
      assert is_binary(first["cursor"])

      second_conn =
        get(conn, "/api/v1/timeline?service=alpha&limit=1&window=24h&cursor=#{first["cursor"]}")

      second = json_response(second_conn, 200)

      assert length(second["events"]) == 1
      assert hd(second["events"])["event"] == "incident.opened"
      assert is_nil(second["cursor"])
    end

    test "paginates correctly when multiple events share the same timestamp", %{conn: conn} do
      timestamp = ts(-10)

      insert_event!(%{
        id: "EVT-a",
        service: "alpha",
        event: "incident.opened",
        created_at: timestamp
      })

      insert_event!(%{
        id: "EVT-b",
        service: "alpha",
        event: "error.new_class",
        created_at: timestamp
      })

      first_conn = get(conn, "/api/v1/timeline?service=alpha&limit=1&window=24h")
      first = json_response(first_conn, 200)

      second_conn =
        get(conn, "/api/v1/timeline?service=alpha&limit=1&window=24h&cursor=#{first["cursor"]}")

      second = json_response(second_conn, 200)

      first_id = hd(first["events"])["id"]
      second_id = hd(second["events"])["id"]

      assert first_id == "EVT-b"
      assert second_id == "EVT-a"
      refute first_id == second_id
    end

    test "returns 401 without auth", %{conn: conn} do
      conn =
        conn
        |> delete_req_header("authorization")
        |> get("/api/v1/timeline")

      assert json_response(conn, 401)["code"] == "invalid_api_key"
    end

    test "returns 422 for invalid window", %{conn: conn} do
      conn = get(conn, "/api/v1/timeline?window=99h")
      body = json_response(conn, 422)

      assert body["code"] == "validation_error"
      assert body["errors"]["window"] == ["must be one of: 1h, 6h, 24h, 7d, 30d"]
    end

    test "returns 422 for invalid limit", %{conn: conn} do
      conn = get(conn, "/api/v1/timeline?limit=999")
      body = json_response(conn, 422)

      assert body["code"] == "validation_error"
      assert body["errors"]["limit"] == ["must be a positive integer no greater than 200"]
    end

    test "returns 422 for invalid cursor", %{conn: conn} do
      conn = get(conn, "/api/v1/timeline?cursor=bogus")
      body = json_response(conn, 422)

      assert body["code"] == "validation_error"
      assert body["errors"]["cursor"] == ["must be a valid pagination cursor"]
    end

    test "returns 422 for malformed-but-decodable cursor values", %{conn: conn} do
      cursor =
        %{"created_at" => 1, "id" => 2}
        |> Jason.encode!()
        |> Base.url_encode64(padding: false)

      conn = get(conn, "/api/v1/timeline?cursor=#{cursor}")
      body = json_response(conn, 422)

      assert body["code"] == "validation_error"
      assert body["errors"]["cursor"] == ["must be a valid pagination cursor"]
    end

    test "returns 422 for invalid service shape", %{conn: conn} do
      conn = get(conn, "/api/v1/timeline", %{"service" => ["alpha"]})
      body = json_response(conn, 422)

      assert body["code"] == "validation_error"
      assert body["errors"]["service"] == ["must be a string"]
    end
  end

  defp insert_event!(attrs) do
    defaults = %{
      id: ID.event_id(),
      service: "alpha",
      event: "error.new_class",
      entity_type: "error_group",
      entity_ref: "group-#{ID.generate()}",
      severity: "error",
      summary: "summary",
      payload: Jason.encode!(%{"event" => "error.new_class"}),
      created_at: ts(-5)
    }

    %ServiceEvent{id: attrs[:id] || defaults.id}
    |> ServiceEvent.changeset(Map.merge(defaults, attrs))
    |> Canary.Repo.insert!()
  end

  defp ts(seconds) do
    DateTime.utc_now()
    |> DateTime.add(seconds, :second)
    |> DateTime.to_iso8601()
  end
end

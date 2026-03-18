defmodule CanaryWeb.ErrorDetailLiveTest do
  use CanaryWeb.ConnCase

  import Phoenix.LiveViewTest
  import Canary.Fixtures

  setup do
    clean_status_tables()

    now = DateTime.utc_now() |> DateTime.to_iso8601()
    group_hash = :crypto.hash(:sha256, "api:RuntimeError") |> Base.encode16(case: :lower)

    error =
      Canary.Repo.insert!(%Canary.Schemas.Error{
        id: "ERR-detail-test",
        service: "api",
        error_class: "RuntimeError",
        message: "something broke badly",
        message_template: "something broke {detail}",
        stack_trace: "  (api 1.0.0) lib/api/handler.ex:42: Api.Handler.call/2",
        context: Jason.encode!(%{"user_id" => "usr_123", "path" => "/foo"}),
        severity: "error",
        environment: "production",
        group_hash: group_hash,
        created_at: now
      })

    Canary.Repo.insert!(%Canary.Schemas.ErrorGroup{
      group_hash: group_hash,
      service: "api",
      error_class: "RuntimeError",
      severity: "error",
      first_seen_at: now,
      last_seen_at: now,
      total_count: 42,
      last_error_id: error.id
    })

    %{error: error, group_hash: group_hash}
  end

  describe "show" do
    test "renders error detail", %{conn: conn, error: error} do
      {:ok, _view, html} = live(conn, "/dashboard/errors/#{error.id}")
      assert html =~ "RuntimeError"
      assert html =~ "something broke badly"
      assert html =~ "api"
      assert html =~ error.id
    end

    test "renders stack trace", %{conn: conn, error: error} do
      {:ok, _view, html} = live(conn, "/dashboard/errors/#{error.id}")
      assert html =~ "Stack Trace"
      assert html =~ "Api.Handler.call/2"
    end

    test "renders context JSON", %{conn: conn, error: error} do
      {:ok, _view, html} = live(conn, "/dashboard/errors/#{error.id}")
      assert html =~ "Context"
      assert html =~ "usr_123"
      assert html =~ "/foo"
    end

    test "renders group summary", %{conn: conn, error: error} do
      {:ok, _view, html} = live(conn, "/dashboard/errors/#{error.id}")
      assert html =~ "Group Summary"
      assert html =~ "42"
    end

    test "redirects on unknown error ID", %{conn: conn} do
      assert {:error, {:live_redirect, %{to: "/dashboard/errors"}}} =
               live(conn, "/dashboard/errors/ERR-nonexistent")
    end
  end
end

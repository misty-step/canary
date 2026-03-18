defmodule CanaryWeb.ErrorsLiveTest do
  use CanaryWeb.ConnCase

  import Phoenix.LiveViewTest
  import Canary.Fixtures

  setup do
    clean_status_tables()
    :ok
  end

  describe "mount and filters" do
    test "renders empty state", %{conn: conn} do
      {:ok, _view, html} = live(conn, "/dashboard/errors")
      assert html =~ "Error Groups"
      assert html =~ "No errors in this window"
    end

    test "lists error groups", %{conn: conn} do
      create_error_group("api", "RuntimeError", 10)
      create_error_group("web", "TypeError", 3)

      {:ok, _view, html} = live(conn, "/dashboard/errors")
      assert html =~ "RuntimeError"
      assert html =~ "TypeError"
      assert html =~ "api"
      assert html =~ "web"
    end

    test "filters by service via URL params", %{conn: conn} do
      create_error_group("api", "RuntimeError", 10)
      create_error_group("web", "TypeError", 3)

      {:ok, _view, html} = live(conn, "/dashboard/errors?service=api")
      assert html =~ "RuntimeError"
      refute html =~ "TypeError"
    end

    test "filters by severity via URL params", %{conn: conn} do
      # create_error_group defaults to severity "error"
      create_error_group("api", "RuntimeError", 10)

      {:ok, _view, html} = live(conn, "/dashboard/errors?severity=warning")
      assert html =~ "No errors in this window"
    end

    test "filters by time window", %{conn: conn} do
      old_time =
        DateTime.utc_now()
        |> DateTime.add(-7200, :second)
        |> DateTime.to_iso8601()

      create_error_group("api", "OldError", 5, last_seen_at: old_time)

      {:ok, _view, html} = live(conn, "/dashboard/errors?window=1h")
      assert html =~ "No errors in this window"

      {:ok, _view, html} = live(conn, "/dashboard/errors?window=24h")
      assert html =~ "OldError"
    end
  end

  describe "filter form interaction" do
    test "changing service filter updates via push_patch", %{conn: conn} do
      create_error_group("api", "RuntimeError", 10)
      create_error_group("web", "TypeError", 3)

      {:ok, view, _html} = live(conn, "/dashboard/errors")

      html =
        view
        |> element("form")
        |> render_change(%{"service" => "api", "severity" => "all", "window" => "24h"})

      # After push_patch, only api errors should show
      assert html =~ "RuntimeError"
    end
  end
end

defmodule CanaryWeb.DashboardLiveTest do
  use CanaryWeb.ConnCase

  import Phoenix.LiveViewTest
  import Canary.Fixtures

  setup do
    clean_status_tables()
    :ok
  end

  describe "mount" do
    test "renders overview with no targets", %{conn: conn} do
      {:ok, _view, html} = live(conn, "/dashboard")
      assert html =~ "Health Targets"
      assert html =~ "No targets configured"
    end

    test "renders targets with state", %{conn: conn} do
      create_target_with_state("api", "up")
      create_target_with_state("web", "down")

      {:ok, _view, html} = live(conn, "/dashboard")
      assert html =~ "api"
      assert html =~ "web"
      assert html =~ "dot-up"
      assert html =~ "dot-down"
    end

    test "renders recent error groups", %{conn: conn} do
      create_error_group("api", "RuntimeError", 5)

      {:ok, _view, html} = live(conn, "/dashboard")
      assert html =~ "RuntimeError"
      assert html =~ "api"
    end
  end

  describe "PubSub" do
    test "new error appears in feed", %{conn: conn} do
      {:ok, view, _html} = live(conn, "/dashboard")

      error = %{
        id: "ERR-test123",
        service: "billing",
        error_class: "PaymentError",
        message: "Card declined",
        severity: "error",
        created_at: DateTime.utc_now() |> DateTime.to_iso8601()
      }

      Phoenix.PubSub.broadcast(Canary.PubSub, "errors:new", {:new_error, struct!(Canary.Schemas.Error, error)})

      html = render(view)
      assert html =~ "PaymentError"
      assert html =~ "billing"
    end
  end

  describe "health polling" do
    test "refreshes targets on :poll_health", %{conn: conn} do
      {:ok, view, html} = live(conn, "/dashboard")
      assert html =~ "No targets configured"

      create_target_with_state("new-svc", "up")
      send(view.pid, :poll_health)

      html = render(view)
      assert html =~ "new-svc"
    end
  end
end

defmodule CanaryWeb.DashboardAuthTest do
  use CanaryWeb.ConnCase

  import Phoenix.LiveViewTest

  @password "test-password-123"
  @auth_secret "test-dashboard-auth-secret"

  setup do
    original_hash = Application.get_env(:canary, :dashboard_password_hash)
    original_version = Application.get_env(:canary, :dashboard_auth_version)
    put_dashboard_password(@password)

    on_exit(fn ->
      Application.put_env(:canary, :dashboard_password_hash, original_hash)
      Application.put_env(:canary, :dashboard_auth_version, original_version)
      :ets.match_delete(:canary_rate_limits, {{:auth_fail, :_}, :_, :_})
    end)

    :ok
  end

  describe "unauthenticated" do
    test "redirects /dashboard to login", %{conn: conn} do
      assert {:error, {:redirect, %{to: "/dashboard/login"}}} = live(conn, "/dashboard")
    end

    test "redirects /dashboard/errors to login", %{conn: conn} do
      assert {:error, {:redirect, %{to: "/dashboard/login"}}} = live(conn, "/dashboard/errors")
    end

    test "login page renders", %{conn: conn} do
      {:ok, _view, html} = live(conn, "/dashboard/login")
      assert html =~ "Authenticate"
      assert html =~ "password"
    end
  end

  describe "login" do
    test "correct password sets session and redirects", %{conn: conn} do
      conn = post(conn, "/dashboard/login", %{"password" => @password})
      assert redirected_to(conn) == "/dashboard"

      conn = get(recycle(conn), "/dashboard")
      assert html_response(conn, 200) =~ "Health Targets"
    end

    test "wrong password redirects back to login", %{conn: conn} do
      conn = post(conn, "/dashboard/login", %{"password" => "wrong"})
      assert redirected_to(conn) == "/dashboard/login"
    end

    test "missing password redirects back to login", %{conn: conn} do
      conn = post(conn, "/dashboard/login", %{})
      assert redirected_to(conn) == "/dashboard/login"
    end
  end

  describe "password rotation" do
    test "old session rejected after password change", %{conn: conn} do
      # Log in with original password
      conn = post(conn, "/dashboard/login", %{"password" => @password})
      assert redirected_to(conn) == "/dashboard"
      conn = recycle(conn)

      # Rotate password
      put_dashboard_password("new-password-456")

      # Old session should be rejected
      assert {:error, {:redirect, %{to: "/dashboard/login"}}} = live(conn, "/dashboard")
    end

    test "same password keeps the session valid across hash regeneration", %{conn: conn} do
      conn = post(conn, "/dashboard/login", %{"password" => @password})
      assert redirected_to(conn) == "/dashboard"
      conn = recycle(conn)

      put_dashboard_password(@password)

      {:ok, _view, html} = live(conn, "/dashboard")
      assert html =~ "Health Targets"
    end

    test "auth version depends on the secret" do
      assert CanaryWeb.DashboardAuth.auth_version(@password, @auth_secret) ==
               CanaryWeb.DashboardAuth.auth_version(@password, @auth_secret)

      refute CanaryWeb.DashboardAuth.auth_version(@password, @auth_secret) ==
               CanaryWeb.DashboardAuth.auth_version(@password, "different-secret")
    end

    test "stale session shows login form instead of redirect loop", %{conn: conn} do
      # Log in
      conn = post(conn, "/dashboard/login", %{"password" => @password})
      conn = recycle(conn)

      # Rotate password
      put_dashboard_password("new-password-456")

      # Login page should show form, not redirect to /dashboard
      {:ok, _view, html} = live(conn, "/dashboard/login")
      assert html =~ "Authenticate"
    end
  end

  describe "rate limiting" do
    test "blocks login after too many failed attempts", %{conn: conn} do
      # Exhaust the rate limit (10 attempts)
      for _ <- 1..10 do
        post(build_conn(), "/dashboard/login", %{"password" => "wrong"})
      end

      # Next attempt should be rate-limited
      conn = post(conn, "/dashboard/login", %{"password" => "wrong"})
      assert redirected_to(conn) == "/dashboard/login"
      assert Phoenix.Flash.get(conn.assigns.flash, :error) =~ "Too many attempts"
    end
  end

  describe "auth disabled" do
    setup do
      Application.put_env(:canary, :dashboard_password_hash, nil)
      Application.put_env(:canary, :dashboard_auth_version, nil)
      :ok
    end

    test "dashboard accessible without login", %{conn: conn} do
      {:ok, _view, html} = live(conn, "/dashboard")
      assert html =~ "Health Targets"
    end

    test "login page redirects to dashboard when auth disabled", %{conn: conn} do
      conn = post(conn, "/dashboard/login", %{"password" => "anything"})
      assert redirected_to(conn) == "/dashboard"
    end
  end

  defp put_dashboard_password(password) do
    Application.put_env(:canary, :dashboard_password_hash, Bcrypt.hash_pwd_salt(password))

    Application.put_env(
      :canary,
      :dashboard_auth_version,
      CanaryWeb.DashboardAuth.auth_version(password, @auth_secret)
    )
  end
end

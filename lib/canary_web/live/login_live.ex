defmodule CanaryWeb.LoginLive do
  use CanaryWeb, :live_view

  @impl true
  def mount(_params, session, socket) do
    if session["dashboard_authenticated"] do
      {:ok, redirect(socket, to: "/dashboard")}
    else
      {:ok, socket}
    end
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div style="display:flex;align-items:center;justify-content:center;min-height:60vh;">
      <div class="card" style="width:100%;max-width:360px;">
        <div class="card-header">Authenticate</div>
        <form method="post" action={~p"/dashboard/login"} style="display:flex;flex-direction:column;gap:12px;margin-top:8px;">
          <input type="hidden" name="_csrf_token" value={Phoenix.Controller.get_csrf_token()} />
          <input
            type="password"
            name="password"
            placeholder="Password"
            autofocus
            autocomplete="current-password"
            style="background:var(--bg);border:1px solid var(--border);border-radius:var(--radius);color:var(--text);padding:8px 12px;font-family:var(--font);font-size:12px;"
          />
          <button
            type="submit"
            style="background:var(--amber);color:var(--bg);border:none;border-radius:var(--radius);padding:8px 12px;font-family:var(--font);font-size:11px;font-weight:600;text-transform:uppercase;letter-spacing:1px;cursor:pointer;"
          >
            Enter
          </button>
        </form>
      </div>
    </div>
    """
  end
end

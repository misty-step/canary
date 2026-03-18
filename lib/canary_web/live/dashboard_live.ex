defmodule CanaryWeb.DashboardLive do
  use CanaryWeb, :live_view

  alias Canary.Schemas.{ErrorGroup, Target, TargetCheck, TargetState}
  import Ecto.Query

  @health_poll_ms 30_000
  @uptime_checks 90
  @error_feed_limit 20

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      Phoenix.PubSub.subscribe(Canary.PubSub, "errors:new")
      :timer.send_interval(@health_poll_ms, self(), :poll_health)
    end

    targets = fetch_targets_with_checks()
    errors = fetch_recent_errors()

    {:ok,
     socket
     |> assign(:page_title, "Overview")
     |> assign(:targets, targets)
     |> assign(:has_errors, errors != [])
     |> stream(:error_feed, errors)}
  end

  @impl true
  def handle_info(:poll_health, socket) do
    {:noreply, assign(socket, :targets, fetch_targets_with_checks())}
  end

  @impl true
  def handle_info({:new_error, error}, socket) do
    entry = %{
      id: error.id,
      service: error.service,
      error_class: error.error_class,
      message: String.slice(error.message, 0, 120),
      severity: error.severity,
      created_at: error.created_at
    }

    {:noreply,
     socket
     |> assign(:has_errors, true)
     |> stream_insert(:error_feed, entry, at: 0, limit: @error_feed_limit)}
  end

  # -- Data fetching --

  defp fetch_targets_with_checks do
    targets =
      from(t in Target,
        left_join: s in TargetState,
        on: t.id == s.target_id,
        order_by: t.name,
        select: {t, s}
      )
      |> repo().all()

    target_ids = Enum.map(targets, fn {t, _} -> t.id end)

    checks_by_target =
      if target_ids == [] do
        %{}
      else
        from(c in TargetCheck,
          where: c.target_id in ^target_ids,
          order_by: [desc: c.checked_at],
          select: %{target_id: c.target_id, result: c.result, checked_at: c.checked_at}
        )
        |> repo().all()
        |> Enum.group_by(& &1.target_id)
        |> Map.new(fn {id, checks} -> {id, Enum.take(checks, @uptime_checks)} end)
      end

    Enum.map(targets, fn {target, state} ->
      checks = Map.get(checks_by_target, target.id, [])

      %{
        id: target.id,
        name: target.name,
        url: target.url,
        state: (state && state.state) || "unknown",
        consecutive_failures: (state && state.consecutive_failures) || 0,
        last_checked_at: state && state.last_checked_at,
        checks: checks
      }
    end)
  end

  defp fetch_recent_errors do
    from(g in ErrorGroup,
      where: g.status == "active",
      order_by: [desc: g.last_seen_at],
      limit: ^@error_feed_limit,
      select: %{
        id: g.group_hash,
        service: g.service,
        error_class: g.error_class,
        message: g.message_template,
        severity: g.severity,
        created_at: g.last_seen_at
      }
    )
    |> repo().all()
  end

  defp repo, do: Canary.Repos.read_repo()

  # -- Template --

  @impl true
  def render(assigns) do
    ~H"""
    <div class="card">
      <div class="card-header">Health Targets</div>
      <div :if={@targets == []} class="empty">No targets configured</div>
      <table :if={@targets != []}>
        <thead>
          <tr>
            <th></th>
            <th>Target</th>
            <th>State</th>
            <th>Failures</th>
            <th>Last Check</th>
            <th>Uptime (last 90)</th>
          </tr>
        </thead>
        <tbody>
          <tr :for={t <- @targets}>
            <td><.status_dot state={t.state} /></td>
            <td>
              <strong><%= t.name %></strong>
              <div class="meta"><%= t.url %></div>
            </td>
            <td><span class={"badge #{state_badge(t.state)}"}><%= t.state %></span></td>
            <td class="count-badge"><%= t.consecutive_failures %></td>
            <td><.time_ago datetime={t.last_checked_at || ""} /></td>
            <td style="min-width:180px"><.uptime_bar checks={t.checks} /></td>
          </tr>
        </tbody>
      </table>
    </div>

    <div class="card section-gap">
      <div class="card-header">Recent Errors</div>
      <div :if={not @has_errors} class="empty">No errors</div>
      <table :if={@has_errors}>
        <thead>
          <tr>
            <th></th>
            <th>Service</th>
            <th>Class</th>
            <th>Message</th>
            <th>When</th>
          </tr>
        </thead>
        <tbody id="error-feed" phx-update="stream">
          <tr :for={{dom_id, error} <- @streams.error_feed} id={dom_id}>
            <td><.severity_badge severity={error.severity} /></td>
            <td><%= error.service %></td>
            <td>
              <a href={"/dashboard/errors?error_class=#{error.error_class}"}><%= error.error_class %></a>
            </td>
            <td class="meta"><%= truncate_message(error.message) %></td>
            <td><.time_ago datetime={error.created_at || ""} /></td>
          </tr>
        </tbody>
      </table>
    </div>
    """
  end

  defp state_badge("up"), do: "badge-green"
  defp state_badge("degraded"), do: "badge-yellow"
  defp state_badge("down"), do: "badge-red"
  defp state_badge(_), do: "badge-muted"

  defp truncate_message(nil), do: ""
  defp truncate_message(msg) when byte_size(msg) > 100, do: String.slice(msg, 0, 100) <> "..."
  defp truncate_message(msg), do: msg
end

defmodule CanaryWeb.ErrorsLive do
  use CanaryWeb, :live_view

  alias Canary.Schemas.ErrorGroup
  import Ecto.Query

  @per_page 25
  @windows ~w(1h 6h 24h 7d 30d)

  @impl true
  def mount(_params, _session, socket) do
    services = fetch_services()
    {:ok, assign(socket, :services, services)}
  end

  @impl true
  def handle_params(params, _uri, socket) do
    filters = parse_filters(params)
    {groups, total} = fetch_groups(filters)
    total_pages = max(1, ceil(total / @per_page))

    {:noreply,
     socket
     |> assign(:page_title, "Errors")
     |> assign(:filters, filters)
     |> assign(:groups, groups)
     |> assign(:total, total)
     |> assign(:total_pages, total_pages)}
  end

  @impl true
  def handle_event("filter", params, socket) do
    query_params =
      %{
        "service" => params["service"],
        "severity" => params["severity"],
        "window" => params["window"],
        "error_class" => params["error_class"]
      }
      |> Enum.reject(fn {_, v} -> v in [nil, "", "all"] end)
      |> Map.new()

    {:noreply, push_patch(socket, to: "/dashboard/errors?" <> URI.encode_query(query_params))}
  end

  # -- Data --

  defp parse_filters(params) do
    %{
      service: params["service"],
      severity: params["severity"],
      window: if(params["window"] in @windows, do: params["window"], else: "24h"),
      error_class: params["error_class"],
      page: parse_page(params["page"])
    }
  end

  defp parse_page(nil), do: 1

  defp parse_page(str) do
    case Integer.parse(str) do
      {n, _} -> max(1, n)
      :error -> 1
    end
  end

  defp fetch_groups(filters) do
    cutoff = window_cutoff(filters.window)

    base =
      from(g in ErrorGroup,
        where: g.last_seen_at >= ^cutoff and g.status == "active",
        order_by: [desc: g.total_count]
      )

    base = if filters.service, do: from(g in base, where: g.service == ^filters.service), else: base

    base =
      if filters.severity,
        do: from(g in base, where: g.severity == ^filters.severity),
        else: base

    base =
      if filters.error_class,
        do: from(g in base, where: g.error_class == ^filters.error_class),
        else: base

    total = repo().aggregate(base, :count)

    groups =
      base
      |> limit(^@per_page)
      |> offset(^((@per_page * (filters.page - 1))))
      |> repo().all()

    {groups, total}
  end

  defp fetch_services do
    from(g in ErrorGroup,
      where: g.status == "active",
      distinct: true,
      select: g.service,
      order_by: g.service
    )
    |> repo().all()
  end

  defp window_cutoff(window) do
    seconds =
      case window do
        "1h" -> 3_600
        "6h" -> 21_600
        "24h" -> 86_400
        "7d" -> 604_800
        "30d" -> 2_592_000
        _ -> 86_400
      end

    DateTime.utc_now()
    |> DateTime.add(-seconds, :second)
    |> DateTime.to_iso8601()
  end

  defp repo, do: Canary.Repos.read_repo()

  defp filter_path(filters) do
    params =
      %{
        "service" => filters.service,
        "severity" => filters.severity,
        "window" => filters.window,
        "error_class" => filters.error_class
      }
      |> Enum.reject(fn {_, v} -> is_nil(v) end)

    "/dashboard/errors?" <> URI.encode_query(params)
  end

  # -- Template --

  @impl true
  def render(assigns) do
    ~H"""
    <div class="card">
      <div class="card-header">Error Groups (<%= @total %>)</div>

      <form phx-change="filter" class="filters">
        <select name="service">
          <option value="all">All services</option>
          <option :for={s <- @services} value={s} selected={@filters.service == s}><%= s %></option>
        </select>

        <select name="severity">
          <option value="all">All severities</option>
          <option value="error" selected={@filters.severity == "error"}>error</option>
          <option value="warning" selected={@filters.severity == "warning"}>warning</option>
          <option value="info" selected={@filters.severity == "info"}>info</option>
        </select>

        <select name="window">
          <option :for={w <- ~w(1h 6h 24h 7d 30d)} value={w} selected={@filters.window == w}><%= w %></option>
        </select>

        <input
          :if={@filters.error_class}
          type="hidden"
          name="error_class"
          value={@filters.error_class}
        />
      </form>

      <div :if={@groups == []} class="empty">No errors in this window</div>

      <table :if={@groups != []}>
        <thead>
          <tr>
            <th></th>
            <th>Service</th>
            <th>Error Class</th>
            <th>Message</th>
            <th>Count</th>
            <th>First Seen</th>
            <th>Last Seen</th>
          </tr>
        </thead>
        <tbody>
          <tr :for={g <- @groups}>
            <td><.severity_badge severity={g.severity} /></td>
            <td><%= g.service %></td>
            <td><a href={"/dashboard/errors/#{g.last_error_id}"}><%= g.error_class %></a></td>
            <td class="meta"><%= truncate(g.message_template, 80) %></td>
            <td class="count-badge"><%= g.total_count %></td>
            <td><.time_ago datetime={g.first_seen_at} /></td>
            <td><.time_ago datetime={g.last_seen_at} /></td>
          </tr>
        </tbody>
      </table>

      <.pagination page={@filters.page} total_pages={@total_pages} patch={filter_path(@filters)} />
    </div>
    """
  end

  defp truncate(nil, _), do: ""
  defp truncate(s, max) when byte_size(s) > max, do: String.slice(s, 0, max) <> "..."
  defp truncate(s, _), do: s
end

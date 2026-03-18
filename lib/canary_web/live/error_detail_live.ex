defmodule CanaryWeb.ErrorDetailLive do
  use CanaryWeb, :live_view

  alias Canary.Schemas.{Error, ErrorGroup}

  @impl true
  def mount(%{"id" => id}, _session, socket) do
    case repo().get(Error, id) do
      nil ->
        {:ok,
         socket
         |> put_flash(:error, "Error not found")
         |> push_navigate(to: "/dashboard/errors")}

      error ->
        group = repo().get(ErrorGroup, error.group_hash)

        {:ok,
         socket
         |> assign(:page_title, error.error_class)
         |> assign(:error, error)
         |> assign(:group, group)
         |> assign(:context, decode_context(error.context))}
    end
  end

  defp decode_context(nil), do: nil

  defp decode_context(json) when is_binary(json) do
    case Jason.decode(json) do
      {:ok, map} -> Jason.encode!(map, pretty: true)
      _ -> json
    end
  end

  defp decode_context(other), do: inspect(other)

  defp repo, do: Canary.Repos.read_repo()

  @impl true
  def render(assigns) do
    ~H"""
    <div class="card">
      <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px">
        <div>
          <div class="card-header" style="margin-bottom:4px"><%= @error.error_class %></div>
          <div class="meta"><%= @error.service %> · <%= @error.environment %></div>
        </div>
        <.severity_badge severity={@error.severity} />
      </div>

      <div style="margin-bottom:16px">
        <strong><%= @error.message %></strong>
      </div>

      <div style="display:flex;gap:24px;margin-bottom:16px" class="meta">
        <span>ID: <code><%= @error.id %></code></span>
        <span>Group: <code><%= @error.group_hash %></code></span>
        <span>Created: <.time_ago datetime={@error.created_at} /></span>
      </div>
    </div>

    <div :if={@group} class="card">
      <div class="card-header">Group Summary</div>
      <div style="display:flex;gap:24px" class="meta">
        <span>Total occurrences: <strong class="count-badge"><%= @group.total_count %></strong></span>
        <span>First seen: <.time_ago datetime={@group.first_seen_at} /></span>
        <span>Last seen: <.time_ago datetime={@group.last_seen_at} /></span>
        <span>Status: <span class={"badge #{group_status_class(@group.status)}"}><%= @group.status %></span></span>
      </div>
    </div>

    <div :if={@error.stack_trace} class="card">
      <div class="card-header">Stack Trace</div>
      <pre><%= @error.stack_trace %></pre>
    </div>

    <div :if={@context} class="card">
      <div class="card-header">Context</div>
      <pre><%= @context %></pre>
    </div>

    <div style="margin-top:16px">
      <a href="/dashboard/errors">&larr; Back to errors</a>
    </div>
    """
  end

  defp group_status_class("active"), do: "badge-red"
  defp group_status_class("resolved"), do: "badge-green"
  defp group_status_class(_), do: "badge-muted"
end

defmodule CanaryWeb.DashboardComponents do
  @moduledoc "Shared function components for the dashboard LiveViews."
  use Phoenix.Component

  attr :state, :string, required: true

  def status_dot(assigns) do
    ~H"""
    <span class={"dot dot-#{@state}"} title={@state}></span>
    """
  end

  attr :checks, :list, required: true
  attr :total, :integer, default: 90

  def uptime_bar(assigns) do
    padded =
      if length(assigns.checks) < assigns.total do
        padding = List.duplicate(:empty, assigns.total - length(assigns.checks))
        padding ++ Enum.reverse(assigns.checks)
      else
        assigns.checks |> Enum.take(assigns.total) |> Enum.reverse()
      end

    assigns = assign(assigns, :ticks, padded)

    ~H"""
    <div class="uptime-bar">
      <span :for={tick <- @ticks} class={"tick #{tick_class(tick)}"}></span>
    </div>
    """
  end

  defp tick_class(:empty), do: "tick-empty"
  defp tick_class(%{result: "success"}), do: "tick-success"
  defp tick_class(%{result: _}), do: "tick-failure"
  defp tick_class(_), do: "tick-empty"

  attr :severity, :string, required: true

  def severity_badge(assigns) do
    ~H"""
    <span class={"badge #{severity_class(@severity)}"}><%= @severity %></span>
    """
  end

  defp severity_class("error"), do: "badge-red"
  defp severity_class("warning"), do: "badge-yellow"
  defp severity_class("info"), do: "badge-muted"
  defp severity_class(_), do: "badge-muted"

  attr :datetime, :string, required: true

  def time_ago(assigns) do
    ~H"""
    <span class="meta" title={@datetime}><%= relative_time(@datetime) %></span>
    """
  end

  defp relative_time(nil), do: "—"

  defp relative_time(iso_string) do
    case DateTime.from_iso8601(iso_string) do
      {:ok, dt, _} ->
        diff = DateTime.diff(DateTime.utc_now(), dt, :second)

        cond do
          diff < 60 -> "#{diff}s ago"
          diff < 3_600 -> "#{div(diff, 60)}m ago"
          diff < 86_400 -> "#{div(diff, 3_600)}h ago"
          true -> "#{div(diff, 86_400)}d ago"
        end

      _ ->
        iso_string
    end
  end

  attr :page, :integer, required: true
  attr :total_pages, :integer, required: true
  attr :patch, :string, required: true

  def pagination(assigns) do
    ~H"""
    <div :if={@total_pages > 1} class="pagination">
      <a :if={@page > 1} patch={"#{@patch}&page=#{@page - 1}"}>Prev</a>
      <span :for={p <- pages_around(@page, @total_pages)} class={if p == @page, do: "current"}>
        <a :if={p != @page} patch={"#{@patch}&page=#{p}"}><%= p %></a>
        <span :if={p == @page}><%= p %></span>
      </span>
      <a :if={@page < @total_pages} patch={"#{@patch}&page=#{@page + 1}"}>Next</a>
    </div>
    """
  end

  defp pages_around(current, total) do
    start = max(1, current - 2)
    stop = min(total, current + 2)
    Enum.to_list(start..stop)
  end
end

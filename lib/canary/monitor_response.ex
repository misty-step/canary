defmodule Canary.MonitorResponse do
  @moduledoc false

  alias Canary.Schemas.Monitor

  @spec render(Monitor.t()) :: map()
  def render(%Monitor{} = monitor) do
    %{
      id: monitor.id,
      name: monitor.name,
      service: Monitor.service_name(monitor),
      mode: monitor.mode,
      expected_every_ms: monitor.expected_every_ms,
      grace_ms: monitor.grace_ms,
      created_at: monitor.created_at
    }
  end
end

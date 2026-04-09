defmodule Canary.Metrics do
  @moduledoc """
  Thin facade around Canary's Prometheus exporter.
  """

  def emit_runtime_metrics do
    CanaryWeb.Telemetry.poll()
  end

  def scrape do
    CanaryWeb.Telemetry.scrape()
  end
end

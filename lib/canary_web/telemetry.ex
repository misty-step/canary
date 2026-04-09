defmodule CanaryWeb.Telemetry do
  use Supervisor
  import Telemetry.Metrics

  alias Canary.Alerter.CircuitBreaker
  alias Canary.Schemas.{Target, TargetState, Webhook}
  import Ecto.Query

  @reporter_name :canary_metrics
  @histogram_buckets [0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2, 5, 10]
  @oban_incomplete_states ~w(available scheduled executing retryable)

  def start_link(arg) do
    Supervisor.start_link(__MODULE__, arg, name: __MODULE__)
  end

  @impl true
  def init(_arg) do
    children = [
      {TelemetryMetricsPrometheus.Core,
       metrics: metrics(), name: @reporter_name, start_async: false},
      # Telemetry poller will execute the given period measurements
      # every 10_000ms. Learn more here: https://hexdocs.pm/telemetry_metrics
      {:telemetry_poller, measurements: periodic_measurements(), period: 10_000}
      # Add reporters as children of your supervision tree.
      # {Telemetry.Metrics.ConsoleReporter, metrics: metrics()}
    ]

    Supervisor.init(children, strategy: :one_for_one)
  end

  def scrape do
    TelemetryMetricsPrometheus.Core.scrape(@reporter_name)
  end

  def poll do
    emit_target_states()

    queue_depths = oban_queue_depths()
    emit_webhook_queue_depth(queue_depths)
    emit_oban_queue_depths(queue_depths)
    emit_circuit_breaker_states()
  end

  def metrics do
    [
      counter("canary.ingest.total", event_name: [:canary, :ingest, :stop]),
      counter("canary.ingest.errors.total",
        event_name: [:canary, :ingest, :stop],
        keep: fn metadata -> metadata.status == :error end
      ),
      distribution("canary.ingest.duration.seconds",
        event_name: [:canary, :ingest, :stop],
        measurement: :duration,
        unit: {:native, :second},
        reporter_options: [buckets: @histogram_buckets]
      ),
      counter("canary.probe.total",
        event_name: [:canary, :probe, :stop],
        tags: [:result]
      ),
      distribution("canary.probe.duration.seconds",
        event_name: [:canary, :probe, :stop],
        measurement: :duration,
        unit: {:millisecond, :second},
        tags: [:result],
        reporter_options: [buckets: @histogram_buckets]
      ),
      last_value("canary.probe.state",
        event_name: [:canary, :probe, :state],
        measurement: :value,
        tags: [:target_id, :state]
      ),
      counter("canary.webhook.delivery.total",
        event_name: [:canary, :webhook, :delivery],
        tags: [:status]
      ),
      last_value("canary.webhook.queue.depth",
        event_name: [:canary, :webhook, :queue],
        measurement: :depth
      ),
      last_value("canary.circuit.breaker.state",
        event_name: [:canary, :circuit_breaker, :state],
        measurement: :value,
        tags: [:webhook_id]
      ),
      last_value("canary.oban.queue.depth",
        event_name: [:canary, :oban, :queue],
        measurement: :depth,
        tags: [:queue]
      ),
      distribution("canary.oban.job.duration.seconds",
        event_name: [:oban, :job, :stop],
        measurement: :duration,
        unit: {:native, :second},
        tags: [:queue, :worker, :state],
        tag_values: fn metadata ->
          %{
            queue: metadata.job.queue,
            worker: metadata.job.worker,
            state: metadata.state
          }
        end,
        reporter_options: [buckets: @histogram_buckets]
      )
    ]
  end

  defp periodic_measurements do
    [
      {__MODULE__, :poll, []}
    ]
  end

  defp emit_target_states do
    from(t in Target,
      left_join: s in TargetState,
      on: s.target_id == t.id,
      select: {t.id, s.state}
    )
    |> Canary.Repos.read_repo().all()
    |> Enum.each(fn {target_id, current_state} ->
      state = current_state || "unknown"

      Enum.each(~w(unknown up degraded down), fn sample_state ->
        :telemetry.execute(
          [:canary, :probe, :state],
          %{value: if(state == sample_state, do: 1, else: 0)},
          %{target_id: target_id, state: sample_state}
        )
      end)
    end)
  end

  defp emit_webhook_queue_depth(queue_depths) do
    :telemetry.execute(
      [:canary, :webhook, :queue],
      %{depth: Map.get(queue_depths, "webhooks", 0)},
      %{}
    )
  end

  defp emit_oban_queue_depths(queue_depths) do
    configured_oban_queues()
    |> Enum.each(fn queue ->
      :telemetry.execute(
        [:canary, :oban, :queue],
        %{depth: Map.get(queue_depths, queue, 0)},
        %{queue: queue}
      )
    end)
  end

  defp emit_circuit_breaker_states do
    Canary.Repos.read_repo().all(Webhook)
    |> Enum.each(fn webhook ->
      :telemetry.execute(
        [:canary, :circuit_breaker, :state],
        %{value: if(CircuitBreaker.open?(webhook.id), do: 1, else: 0)},
        %{webhook_id: webhook.id}
      )
    end)
  end

  defp oban_queue_depths do
    from(j in Oban.Job,
      where: j.state in ^@oban_incomplete_states,
      group_by: j.queue,
      select: {j.queue, count(j.id)}
    )
    |> Canary.Repos.read_repo().all()
    |> Map.new()
  end

  defp configured_oban_queues do
    :canary
    |> Application.fetch_env!(Oban)
    |> Keyword.get(:queues, [])
    |> Keyword.keys()
    |> Enum.map(&to_string/1)
  end
end

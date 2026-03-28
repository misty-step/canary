defmodule Canary.Workers.TlsScan do
  @moduledoc """
  Daily scan of all HTTPS targets for TLS certificate expiry.
  Fires health_check.tls_expiring webhook if cert expires in <14 days.
  """

  use Oban.Worker, queue: :maintenance, max_attempts: 2

  alias Canary.{Timeline, Workers.WebhookDelivery}
  alias Canary.Schemas.{Target, TargetCheck}
  import Ecto.Query

  require Logger

  @expiry_warning_days 14

  @impl Oban.Worker
  def perform(_job) do
    targets =
      from(t in Target, where: t.active == 1 and like(t.url, "https://%"))
      |> Canary.Repos.read_repo().all()

    Enum.each(targets, &check_tls_expiry/1)
    :ok
  end

  defp check_tls_expiry(target) do
    latest_check =
      from(c in TargetCheck,
        where: c.target_id == ^target.id and not is_nil(c.tls_expires_at),
        order_by: [desc: c.checked_at],
        limit: 1
      )
      |> Canary.Repos.read_repo().one()

    case latest_check do
      %{tls_expires_at: expiry} when is_binary(expiry) ->
        check_expiry_date(target, expiry)

      _ ->
        :ok
    end
  end

  defp check_expiry_date(target, expiry_str) do
    with {:ok, expiry, _} <- DateTime.from_iso8601(expiry_str) do
      days_until = DateTime.diff(expiry, DateTime.utc_now(), :day)

      if days_until < @expiry_warning_days and days_until >= 0 do
        Logger.warning("TLS cert for #{target.name} expires in #{days_until} days")
        now = DateTime.utc_now() |> DateTime.to_iso8601()
        payload = Timeline.record_tls_expiring!(target, expiry_str, days_until, now)
        WebhookDelivery.enqueue_for_event("health_check.tls_expiring", payload)
      end
    end
  end
end

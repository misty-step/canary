defmodule Canary.TargetResponse do
  @moduledoc false

  alias Canary.Schemas.Target

  @spec render(Target.t()) :: map()
  def render(%Target{} = target) do
    %{
      id: target.id,
      name: target.name,
      service: Target.service_name(target),
      url: target.url,
      method: target.method,
      interval_ms: target.interval_ms,
      timeout_ms: target.timeout_ms,
      expected_status: target.expected_status,
      active: Target.active?(target),
      created_at: target.created_at
    }
  end
end

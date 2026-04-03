defmodule Canary.IncidentsErrorShapeTest do
  @moduledoc """
  Tests the error-shape contract of Incidents.correlate/3.
  Runs async so that spawned tasks do NOT inherit sandbox access,
  triggering a real DBConnection.OwnershipError inside the rescue clause.
  """
  use Canary.DataCase, async: true

  alias Canary.Incidents

  test "correlate/3 returns {:error, {:exception, _}} when an internal exception occurs" do
    ref = make_ref()
    test_pid = self()

    # Raw spawn — no $callers propagation, so sandbox denies DB access.
    spawn(fn ->
      result = Incidents.correlate(:health_transition, "TGT-ghost", "ghost")
      send(test_pid, {ref, result})
    end)

    assert_receive {^ref, {:error, {:exception, DBConnection.OwnershipError}}}, 5_000
  end
end

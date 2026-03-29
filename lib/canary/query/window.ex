defmodule Canary.Query.Window do
  @moduledoc false

  @allowed_windows ~w(1h 6h 24h 7d 30d)

  @spec to_cutoff(String.t()) :: {:ok, String.t()} | {:error, :invalid_window}
  def to_cutoff(window) when window in @allowed_windows do
    seconds =
      case window do
        "1h" -> 3_600
        "6h" -> 21_600
        "24h" -> 86_400
        "7d" -> 604_800
        "30d" -> 2_592_000
      end

    cutoff =
      DateTime.utc_now()
      |> DateTime.add(-seconds, :second)
      |> DateTime.to_iso8601()

    {:ok, cutoff}
  end

  def to_cutoff(_window), do: {:error, :invalid_window}
end

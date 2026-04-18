defmodule Canary.ID do
  @moduledoc false

  @spec generate(String.t() | nil) :: String.t()
  def generate(prefix \\ nil) do
    nano = Nanoid.generate(12, "0123456789abcdefghijklmnopqrstuvwxyz")

    case prefix do
      nil -> nano
      p -> "#{p}-#{nano}"
    end
  end

  @spec error_id() :: String.t()
  def error_id, do: generate("ERR")
  @spec incident_id() :: String.t()
  def incident_id, do: generate("INC")
  @spec event_id() :: String.t()
  def event_id, do: generate("EVT")
  @spec target_id() :: String.t()
  def target_id, do: generate("TGT")
  @spec monitor_id() :: String.t()
  def monitor_id, do: generate("MON")
  @spec check_in_id() :: String.t()
  def check_in_id, do: generate("CHK")
  @spec webhook_id() :: String.t()
  def webhook_id, do: generate("WHK")
  @spec key_id() :: String.t()
  def key_id, do: generate("KEY")
  @spec annotation_id() :: String.t()
  def annotation_id, do: generate("ANN")
end

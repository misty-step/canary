defmodule Canary.ID do
  @moduledoc false

  def generate(prefix \\ nil) do
    nano = Nanoid.generate(12, "0123456789abcdefghijklmnopqrstuvwxyz")

    case prefix do
      nil -> nano
      p -> "#{p}-#{nano}"
    end
  end

  def error_id, do: generate("ERR")
  def target_id, do: generate("TGT")
  def webhook_id, do: generate("WHK")
  def key_id, do: generate("KEY")
end

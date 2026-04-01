defmodule Canary.CorrelationErrorTag do
  @moduledoc """
  Formats correlation error reasons into human-readable log tags.
  """

  @spec format(term()) :: String.t()
  def format({:exception, module}) when is_atom(module), do: Atom.to_string(module)
  def format({kind, reason}), do: "#{kind}:#{format(reason)}"
  def format(reason) when is_atom(reason), do: Atom.to_string(reason)
  def format(%module{}) when is_atom(module), do: Atom.to_string(module)
  def format(_reason), do: "unexpected"
end

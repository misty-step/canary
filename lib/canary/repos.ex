defmodule Canary.Repos do
  @moduledoc "Repo routing. Returns Repo in test for sandbox compatibility."

  @spec read_repo() :: module()
  def read_repo, do: Application.get_env(:canary, :read_repo, Canary.ReadRepo)
end

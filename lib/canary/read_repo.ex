defmodule Canary.ReadRepo do
  use Ecto.Repo,
    otp_app: :canary,
    adapter: Ecto.Adapters.SQLite3,
    read_only: true
end

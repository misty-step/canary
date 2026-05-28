defmodule Bypass do
  @moduledoc false

  defstruct [:agent, :port, :server]

  def open do
    {:ok, agent} = Agent.start_link(fn -> %{} end)
    port = open_port()
    {:ok, server} = Bandit.start_link(plug: {__MODULE__.Plug, agent}, port: port)

    %__MODULE__{agent: agent, port: port, server: server}
  end

  def expect_once(%__MODULE__{} = bypass, method, path, fun) do
    put_route(bypass, method, path, {:once, fun})
  end

  def expect(%__MODULE__{} = bypass, method, path, fun) do
    put_route(bypass, method, path, {:many, fun})
  end

  def stub(%__MODULE__{} = bypass, method, path, fun) do
    put_route(bypass, method, path, {:many, fun})
  end

  def down(%__MODULE__{server: server}) do
    if Process.alive?(server), do: GenServer.stop(server)
    :ok
  end

  defp put_route(%__MODULE__{agent: agent}, method, path, expectation) do
    Agent.update(agent, &Map.put(&1, key(method, path), expectation))
    :ok
  end

  defp key(method, path), do: {String.upcase(method), path}

  defp open_port do
    {:ok, socket} = :gen_tcp.listen(0, [:binary, active: false, reuseaddr: true])
    {:ok, port} = :inet.port(socket)
    :ok = :gen_tcp.close(socket)
    port
  end

  defmodule Plug do
    @moduledoc false

    def init(agent), do: agent

    def call(conn, agent) do
      route_key = {conn.method, conn.request_path}

      fun =
        Agent.get_and_update(agent, fn routes ->
          case Map.fetch(routes, route_key) do
            {:ok, {:once, fun}} -> {fun, Map.delete(routes, route_key)}
            {:ok, {:many, fun}} -> {fun, routes}
            :error -> {nil, routes}
          end
        end)

      case fun do
        nil -> raise "route error"
        fun -> fun.(conn)
      end
    end
  end
end

defmodule Canary.Health.SSRFGuard do
  @moduledoc """
  Validates URLs against SSRF attacks. Resolves hostname, checks all
  returned IPs against blocked ranges. Re-validates after redirects.
  """

  import Bitwise

  @blocked_ranges [
    # Loopback
    {{127, 0, 0, 0}, 8},
    # Private 10.x
    {{10, 0, 0, 0}, 8},
    # Private 172.16-31.x
    {{172, 16, 0, 0}, 12},
    # Private 192.168.x
    {{192, 168, 0, 0}, 16},
    # Link-local
    {{169, 254, 0, 0}, 16},
    # Current network
    {{0, 0, 0, 0}, 8}
  ]

  @spec validate_url(String.t(), boolean()) :: :ok | {:error, String.t()}
  def validate_url(url, allow_private \\ false) do
    with {:ok, uri} <- parse_uri(url),
         :ok <- validate_scheme(uri),
         :ok <- validate_host(uri, allow_private) do
      :ok
    end
  end

  defp parse_uri(url) do
    case URI.parse(url) do
      %URI{host: nil} -> {:error, "missing host"}
      %URI{host: ""} -> {:error, "empty host"}
      uri -> {:ok, uri}
    end
  end

  defp validate_scheme(%URI{scheme: scheme}) when scheme in ["http", "https"], do: :ok
  defp validate_scheme(_), do: {:error, "scheme must be http or https"}

  defp validate_host(%URI{host: host}, allow_private) do
    case resolve_host(host) do
      {:ok, ips} ->
        blocked = Enum.find(ips, &ip_blocked?(&1, allow_private))

        if blocked do
          {:error, "blocked IP: #{format_ip(blocked)}"}
        else
          :ok
        end

      {:error, reason} ->
        {:error, "DNS resolution failed: #{inspect(reason)}"}
    end
  end

  defp resolve_host(host) do
    host_charlist = String.to_charlist(host)

    case :inet.getaddrs(host_charlist, :inet) do
      {:ok, v4} ->
        case :inet.getaddrs(host_charlist, :inet6) do
          {:ok, v6} -> {:ok, v4 ++ v6}
          _ -> {:ok, v4}
        end

      {:error, _} ->
        case :inet.getaddrs(host_charlist, :inet6) do
          {:ok, v6} -> {:ok, v6}
          {:error, reason} -> {:error, reason}
        end
    end
  end

  defp ip_blocked?(_ip, true), do: false

  defp ip_blocked?(ip, false) when tuple_size(ip) == 4 do
    Enum.any?(@blocked_ranges, fn {network, prefix_len} ->
      in_cidr?(ip, network, prefix_len)
    end)
  end

  defp ip_blocked?(ip, false) when tuple_size(ip) == 8 do
    ip == {0, 0, 0, 0, 0, 0, 0, 1} or
      (elem(ip, 0) == 0xFE80)
  end

  defp ip_blocked?(_, _), do: false

  defp in_cidr?(ip, network, prefix_len) do
    ip_int = ip_to_integer(ip)
    net_int = ip_to_integer(network)
    mask = Bitwise.bnot((1 <<< (32 - prefix_len)) - 1) &&& 0xFFFFFFFF
    (ip_int &&& mask) == (net_int &&& mask)
  end

  defp ip_to_integer({a, b, c, d}), do: a <<< 24 ||| b <<< 16 ||| c <<< 8 ||| d

  defp format_ip(ip) when tuple_size(ip) == 4, do: :inet.ntoa(ip) |> to_string()
  defp format_ip(ip) when tuple_size(ip) == 8, do: :inet.ntoa(ip) |> to_string()
end

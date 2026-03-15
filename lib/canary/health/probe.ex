defmodule Canary.Health.Probe do
  @moduledoc """
  Executes HTTP health check probes via shared Finch pool.
  Returns structured result with timing, status, TLS metadata.
  """

  alias Canary.Schemas.Target

  @type result :: %{
          status_code: integer() | nil,
          latency_ms: integer(),
          result: String.t(),
          tls_expires_at: String.t() | nil,
          error_detail: String.t() | nil
        }

  @spec check(Target.t()) :: {:ok, result()} | {:error, result()}
  def check(%Target{} = target) do
    start = System.monotonic_time(:millisecond)
    headers = Target.parsed_headers(target)
    method = String.downcase(target.method) |> String.to_existing_atom()

    req_opts = [
      method: method,
      url: target.url,
      headers: headers,
      receive_timeout: target.timeout_ms,
      redirect: true,
      max_redirects: 3,
      retry: false,
      finch: Canary.Finch
    ]

    case Req.request(req_opts) do
      {:ok, %Req.Response{status: status} = resp} ->
        latency = System.monotonic_time(:millisecond) - start
        tls_expires = extract_tls_expiry(target.url)

        result = evaluate_response(status, resp.body, target)

        outcome = %{
          status_code: status,
          latency_ms: latency,
          result: result,
          tls_expires_at: tls_expires,
          error_detail: nil
        }

        if result == "success", do: {:ok, outcome}, else: {:error, outcome}

      {:error, %Req.TransportError{reason: :timeout}} ->
        latency = System.monotonic_time(:millisecond) - start

        {:error,
         %{
           status_code: nil,
           latency_ms: latency,
           result: "timeout",
           tls_expires_at: nil,
           error_detail: "request timed out after #{target.timeout_ms}ms"
         }}

      {:error, %Req.TransportError{reason: reason}} ->
        latency = System.monotonic_time(:millisecond) - start
        category = categorize_transport_error(reason)

        {:error,
         %{
           status_code: nil,
           latency_ms: latency,
           result: category,
           tls_expires_at: nil,
           error_detail: inspect(reason)
         }}

      {:error, error} ->
        latency = System.monotonic_time(:millisecond) - start

        {:error,
         %{
           status_code: nil,
           latency_ms: latency,
           result: "connection_error",
           tls_expires_at: nil,
           error_detail: inspect(error)
         }}
    end
  end

  defp evaluate_response(status, body, target) do
    cond do
      not status_matches?(status, target.expected_status) ->
        "status_mismatch"

      target.body_contains && not String.contains?(body || "", target.body_contains) ->
        "body_mismatch"

      true ->
        "success"
    end
  end

  @doc "Check if status code matches expected pattern: '200', '200-299', '200,204'"
  def status_matches?(status, expected) do
    cond do
      String.contains?(expected, "-") ->
        [lo, hi] = String.split(expected, "-") |> Enum.map(&String.to_integer/1)
        status >= lo and status <= hi

      String.contains?(expected, ",") ->
        codes =
          String.split(expected, ",")
          |> Enum.map(&String.trim/1)
          |> Enum.map(&String.to_integer/1)

        status in codes

      true ->
        status == String.to_integer(expected)
    end
  end

  defp extract_tls_expiry("https://" <> _ = url) do
    uri = URI.parse(url)
    host = String.to_charlist(uri.host)
    port = uri.port || 443

    with {:ok, sock} <- :ssl.connect(host, port, [verify: :verify_none, depth: 0], 5_000),
         {:ok, cert_der} <- :ssl.peercert(sock),
         :ok <- :ssl.close(sock),
         {:ok, expiry} <- extract_not_after(cert_der) do
      expiry
    else
      _ -> nil
    end
  rescue
    _ -> nil
  end

  defp extract_tls_expiry(_), do: nil

  defp extract_not_after(cert_der) do
    otp_cert = :public_key.pkix_decode_cert(cert_der, :otp)

    validity =
      elem(otp_cert, 1)
      |> elem(1)
      |> elem(4)

    not_after = elem(validity, 1)

    case not_after do
      {:utcTime, time} ->
        {:ok, parse_asn1_time(time)}

      {:generalTime, time} ->
        {:ok, parse_asn1_time(time)}

      _ ->
        {:error, :unknown_format}
    end
  rescue
    _ -> {:error, :parse_failed}
  end

  defp parse_asn1_time(time) when is_list(time) do
    to_string(time)
  end

  defp parse_asn1_time(time), do: to_string(time)

  defp categorize_transport_error(reason) do
    reason_str = inspect(reason) |> String.downcase()

    cond do
      String.contains?(reason_str, "dns") or String.contains?(reason_str, "nxdomain") ->
        "dns_error"

      String.contains?(reason_str, "tls") or String.contains?(reason_str, "ssl") or
          String.contains?(reason_str, "certificate") ->
        "tls_error"

      true ->
        "connection_error"
    end
  end
end

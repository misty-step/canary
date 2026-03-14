defmodule Canary.Errors.Grouping do
  @moduledoc """
  Error grouping: message template stripping, stack trace fingerprinting,
  group hash computation. Three strategies in priority order:
  1. Client fingerprint (explicit)
  2. Stack trace fingerprint (>= 2 in-project frames)
  3. Message template (fallback)
  """

  @normalization_rules [
    # 1. UUIDs
    {~r/\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/i, "<uuid>"},
    # 2. ISO 8601 timestamps
    {~r/\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?(?:Z|[+-]\d{2}:?\d{2})?\b/, "<timestamp>"},
    # 3. Email addresses
    {~r/\b[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}\b/, "<email>"},
    # 4. File paths (Unix)
    {~r{(?:^|[\s('"=])(/[^\s'")\]]+)}, "<path>"},
    # 5. Long hex strings (>8 chars)
    {~r/\b(?:0x)?[0-9a-f]{9,}\b/i, "<hex>"},
    # 6. Integers with 4+ digits
    {~r/\b\d{4,}\b/, "<int>"},
    # 7. Collapse whitespace
    {~r/\s+/, " "}
  ]

  @spec compute_group_hash(map()) :: {String.t(), String.t() | nil}
  def compute_group_hash(attrs) do
    template = strip_template(attrs["message"] || "")

    group_hash =
      cond do
        fingerprint = attrs["fingerprint"] ->
          fingerprint_hash(attrs["service"], fingerprint)

        stack_hash = stack_trace_hash(attrs) ->
          stack_hash

        true ->
          template_hash(attrs["service"], attrs["error_class"], template)
      end

    {group_hash, template}
  end

  @spec strip_template(String.t()) :: String.t()
  def strip_template(message) do
    Enum.reduce(@normalization_rules, message, fn {pattern, replacement}, acc ->
      Regex.replace(pattern, acc, replacement)
    end)
    |> String.trim()
  end

  defp fingerprint_hash(service, fingerprint) when is_list(fingerprint) do
    input = service <> Enum.join(fingerprint, ":")
    sha256(input)
  end

  defp fingerprint_hash(service, fingerprint) when is_binary(fingerprint) do
    fingerprint_hash(service, [fingerprint])
  end

  defp stack_trace_hash(attrs) do
    with stack when is_binary(stack) <- attrs["stack_trace"],
         frames when length(frames) >= 2 <- extract_in_project_frames(stack, attrs) do
      fp =
        frames
        |> Enum.take(5)
        |> Enum.map(&strip_line_number/1)
        |> Enum.join("|")

      sha256(attrs["service"] <> attrs["error_class"] <> fp)
    else
      _ -> nil
    end
  end

  defp template_hash(service, error_class, template) do
    sha256((service || "") <> (error_class || "") <> (template || ""))
  end

  defp extract_in_project_frames(stack_trace, _attrs) do
    module_prefixes = Application.get_env(:canary, :in_project_module_prefixes, [])
    path_prefixes = Application.get_env(:canary, :in_project_path_prefixes, [])

    frames =
      stack_trace
      |> String.split("\n")
      |> Enum.map(&String.trim/1)
      |> Enum.reject(&(&1 == ""))

    in_project =
      Enum.filter(frames, fn frame ->
        Enum.any?(module_prefixes, &String.contains?(frame, &1)) or
          Enum.any?(path_prefixes, &String.contains?(frame, &1))
      end)

    if length(in_project) >= 2 do
      in_project
    else
      Enum.take(frames, 5)
      |> case do
        frames when length(frames) >= 2 -> frames
        _ -> []
      end
    end
  end

  defp strip_line_number(frame) do
    Regex.replace(~r/:\d+/, frame, "")
  end

  defp sha256(input) do
    :crypto.hash(:sha256, input) |> Base.encode16(case: :lower)
  end
end

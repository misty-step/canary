defmodule Canary.Checks.PreloadThenTake do
  use Credo.Check,
    category: :warning,
    base_priority: :high,
    explanations: [
      check: """
      Canary read models must not advertise bounded payloads while loading
      unbounded has-many preloads and trimming them in memory.

      Push collection caps into SQL with a limited preload query, or split the
      read model into an explicit count query plus a limited fetch query.
      """
    ]

  alias Credo.{IssueMeta, SourceFile}
  alias Credo.Execution.ExecutionIssues

  @doc false
  @impl true
  def run_on_all_source_files(exec, source_files, params) do
    Enum.each(source_files, fn source_file ->
      source_file
      |> issues_for_source_file(params)
      |> then(&ExecutionIssues.append(exec, &1))
    end)

    :ok
  end

  @doc false
  @impl true
  def run(%SourceFile{} = _source_file, _params), do: []

  defp issues_for_source_file(%SourceFile{} = source_file, params) do
    if read_model_path?(source_file.filename) do
      source_file
      |> Credo.Code.prewalk(&walk/2, initial_state(source_file, params))
      |> Map.fetch!(:issues)
      |> Enum.reverse()
    else
      []
    end
  end

  defp initial_state(source_file, params) do
    %{
      issue_meta: IssueMeta.for(source_file, params),
      issues: [],
      reported: MapSet.new()
    }
  end

  defp walk({:|>, _, _} = ast, state) do
    {ast, maybe_report_pipe(ast, state)}
  end

  defp walk(ast, state), do: {ast, state}

  defp maybe_report_pipe(ast, state) do
    stages = flatten_pipe(ast)

    stages
    |> truncations_by_stage()
    |> Enum.reduce(state, fn {truncation, truncation_index}, acc ->
      maybe_report_truncation(stages, truncation, truncation_index, acc)
    end)
  end

  defp maybe_report_truncation(stages, truncation, truncation_index, state) do
    with {:ok, field} <- preloaded_field_for_truncation(stages, truncation_index),
         false <- MapSet.member?(state.reported, {field, truncation.line_no}) do
      state
      |> add_issue(issue_for(state.issue_meta, field, truncation, truncation.line_no))
      |> update_in([:reported], &MapSet.put(&1, {field, truncation.line_no}))
    else
      _ -> state
    end
  end

  defp flatten_pipe({:|>, _, [left, right]}), do: flatten_pipe(left) ++ [right]
  defp flatten_pipe(ast), do: [ast]

  defp truncations_by_stage(stages) do
    stages
    |> Enum.with_index()
    |> Enum.flat_map(fn {stage, index} ->
      case truncation_info(stage) do
        {:ok, truncation} -> [{truncation, index}]
        :error -> []
      end
    end)
  end

  defp preloaded_field_for_truncation(stages, truncation_index) do
    stages
    |> Enum.take(truncation_index)
    |> Enum.with_index()
    |> Enum.find_value(:error, fn {stage, index} ->
      stage
      |> unbounded_preload_fields()
      |> Enum.find_value(fn field ->
        if field_referenced_before_truncation?(stages, index, truncation_index, field) do
          {:ok, field}
        end
      end)
    end)
    |> case do
      {:ok, field} -> {:ok, field}
      :error -> :error
    end
  end

  defp field_referenced_before_truncation?(stages, preload_index, truncation_index, field) do
    stages
    |> Enum.slice(preload_index + 1, truncation_index - preload_index)
    |> Enum.any?(&references_field?(&1, field))
  end

  defp unbounded_preload_fields(stage) do
    stage
    |> preload_specs()
    |> Enum.reject(fn {_field, bounded?} -> bounded? end)
    |> Enum.map(fn {field, _bounded?} -> field end)
  end

  defp preload_specs({{:., _, [module_ast, :preload]}, _meta, args}) when is_list(args) do
    if repo_module?(module_ast), do: args |> List.last() |> preload_specs_for_arg(), else: []
  end

  defp preload_specs({:preload, _meta, args}) when is_list(args) do
    if ecto_preload_macro_args?(args),
      do: args |> List.last() |> preload_specs_for_arg(),
      else: []
  end

  defp preload_specs(_stage), do: []

  defp ecto_preload_macro_args?([binding_ast, _specs]) when is_list(binding_ast),
    do: not Keyword.keyword?(binding_ast)

  defp ecto_preload_macro_args?(_args), do: false

  defp preload_specs_for_arg(field) when is_atom(field), do: [{field, false}]

  defp preload_specs_for_arg(fields) when is_list(fields) do
    Enum.flat_map(fields, fn
      {field, query_ast} when is_atom(field) ->
        [{field, contains_limit?(query_ast)}]

      field when is_atom(field) ->
        [{field, false}]

      _ ->
        []
    end)
  end

  defp preload_specs_for_arg(_arg), do: []

  defp truncation_info(nil), do: :error

  defp truncation_info(ast) do
    {_ast, truncation} =
      Macro.prewalk(ast, nil, fn
        node, nil -> {node, truncation_call(node)}
        node, truncation -> {node, truncation}
      end)

    case truncation do
      nil -> :error
      truncation -> {:ok, truncation}
    end
  end

  defp truncation_call({{:., meta, [module_ast, function]}, call_meta, args})
       when is_list(args) do
    with {:ok, module} <- truncation_module(module_ast),
         true <- truncation_function?(module, function, length(args)) do
      %{
        call: "#{inspect(module)}.#{function}/#{pipeline_arity(function, length(args))}",
        line_no: call_meta[:line] || meta[:line] || 1
      }
    else
      _ -> nil
    end
  end

  defp truncation_call(_node), do: nil

  defp truncation_module({:__aliases__, _, [:Enum]}), do: {:ok, Enum}
  defp truncation_module({:__aliases__, _, [:Stream]}), do: {:ok, Stream}
  defp truncation_module(_module_ast), do: :error

  defp truncation_function?(Enum, :take, arity), do: arity in [1, 2]
  defp truncation_function?(Enum, :slice, arity), do: arity in [2, 3]
  defp truncation_function?(Stream, :take, arity), do: arity in [1, 2]
  defp truncation_function?(_module, _function, _arity), do: false

  defp pipeline_arity(:take, 1), do: 2
  defp pipeline_arity(:slice, 2), do: 3
  defp pipeline_arity(_function, arity), do: arity

  defp references_field?(ast, field) do
    {_ast, found?} =
      Macro.prewalk(ast, false, fn node, found? ->
        {node, found? or field_access?(node, field)}
      end)

    found?
  end

  defp field_access?({{:., _, [module_ast, function]}, _meta, args}, field)
       when function in [:fetch!, :get, :update, :update!] and is_list(args) do
    map_module?(module_ast) and
      Enum.any?(args, &field_key?(&1, field))
  end

  defp field_access?({{:., _, [_object_ast, accessed_field]}, _meta, []}, field),
    do: accessed_field == field

  defp field_access?(_node, _field), do: false

  defp map_module?({:__aliases__, _, [:Map]}), do: true
  defp map_module?(_module_ast), do: false

  defp field_key?(key, field), do: key == field or key == Atom.to_string(field)

  defp contains_limit?({:from, _meta, args}) when is_list(args),
    do: Enum.any?(args, &from_limit_arg?/1)

  defp contains_limit?({:limit, _meta, args}) when is_list(args),
    do: length(args) in [1, 2]

  defp contains_limit?({:|>, _meta, [left, right]}),
    do: contains_limit?(left) or contains_limit?(right)

  defp contains_limit?(_ast), do: false

  defp from_limit_arg?(arg) when is_list(arg),
    do: Keyword.keyword?(arg) and Keyword.has_key?(arg, :limit)

  defp from_limit_arg?(_arg), do: false

  defp repo_module?({:__aliases__, _, parts}), do: List.last(parts) == :Repo
  defp repo_module?(_module_ast), do: false

  defp read_model_path?(filename) when is_binary(filename) do
    String.contains?(filename, "/lib/canary/query/") or
      String.starts_with?(filename, "lib/canary/query/") or
      String.ends_with?(filename, "/lib/canary/query.ex") or
      filename == "lib/canary/query.ex" or
      Regex.match?(~r{(^|/)lib/canary/.*/query[^/]*\.ex$}, filename)
  end

  defp read_model_path?(_filename), do: false

  defp issue_for(issue_meta, field, truncation, line_no) do
    field_name = Atom.to_string(field)

    format_issue(
      issue_meta,
      message:
        "Bounded-payload antipattern at L#{line_no}: preload on `:#{field_name}` followed by " <>
          "#{truncation.call} loads every row into memory before discarding most. " <>
          "Push the cap into SQL: either `preload: [#{field_name}: ^from(r in Rel, order_by: ..., limit: ^max)]`, " <>
          "or split into `count_#{field_name}/1` + `fetch_top_#{field_name}/2`. " <>
          "See `lib/canary/query/incidents.ex:fetch_top_signals/3` for the reference shape.",
      trigger: field_name,
      line_no: line_no
    )
  end

  defp add_issue(state, issue) do
    %{state | issues: [issue | state.issues]}
  end
end

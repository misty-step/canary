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

  @function_definitions [:def, :defp, :defmacro, :defmacrop]

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
      |> SourceFile.ast()
      |> traverse(initial_state(source_file, params))
      |> Map.fetch!(:issues)
      |> Enum.reverse()
    else
      []
    end
  end

  defp initial_state(source_file, params) do
    %{
      ecto_query_aliases: MapSet.new(),
      ecto_query_imported?: false,
      issue_meta: IssueMeta.for(source_file, params),
      local_preload_arities: MapSet.new(),
      issues: [],
      reported: MapSet.new()
    }
  end

  defp traverse(ast, state) when is_list(ast) do
    Enum.reduce(ast, state, &traverse/2)
  end

  defp traverse({:defmodule, _meta, [_module, [do: body]]}, state) do
    state
    |> module_state(body)
    |> then(&traverse(body, &1))
    |> merge_scoped_state(state)
  end

  defp traverse({kind, _meta, args}, state)
       when kind in @function_definitions and is_list(args) do
    args
    |> function_body()
    |> traverse(state)
    |> merge_scoped_state(state)
  end

  defp traverse({:import, _, [{:__aliases__, _, [:Ecto, :Query]} | _args]}, state) do
    %{state | ecto_query_imported?: true}
  end

  defp traverse({:alias, _, [{:__aliases__, _, [:Ecto, :Query]} | args]}, state) do
    %{state | ecto_query_aliases: MapSet.put(state.ecto_query_aliases, ecto_query_alias(args))}
  end

  defp traverse({:|>, _, _} = ast, state) do
    ast
    |> maybe_report_pipe(state)
    |> then(&traverse_children(ast, &1))
  end

  defp traverse(ast, state) when is_tuple(ast) do
    ast
    |> Tuple.to_list()
    |> traverse(state)
  end

  defp traverse(_ast, state), do: state

  defp traverse_children(ast, state) do
    ast
    |> Tuple.to_list()
    |> traverse(state)
  end

  defp module_state(state, body) do
    %{
      state
      | ecto_query_aliases: MapSet.new(),
        ecto_query_imported?: false,
        local_preload_arities: local_preload_arities(body)
    }
  end

  defp merge_scoped_state(scoped_state, parent_state) do
    %{parent_state | issues: scoped_state.issues, reported: scoped_state.reported}
  end

  defp function_body([_head, body]) when is_list(body), do: Keyword.get(body, :do)
  defp function_body(_args), do: nil

  defp ecto_query_alias([[as: {:__aliases__, _, alias_parts}]]), do: alias_parts
  defp ecto_query_alias(_args), do: [:Query]

  defp local_preload_arities(ast), do: collect_local_preload_arities(ast, MapSet.new())

  defp collect_local_preload_arities(ast, arities) when is_list(ast) do
    Enum.reduce(ast, arities, &collect_local_preload_arities/2)
  end

  defp collect_local_preload_arities({:defmodule, _meta, _args}, arities), do: arities

  defp collect_local_preload_arities({kind, _meta, args}, arities)
       when kind in @function_definitions and is_list(args) do
    case args do
      [head | _body] -> put_preload_arity(arities, head)
      _args -> arities
    end
  end

  defp collect_local_preload_arities(ast, arities) when is_tuple(ast) do
    ast
    |> Tuple.to_list()
    |> collect_local_preload_arities(arities)
  end

  defp collect_local_preload_arities(_ast, arities), do: arities

  defp put_preload_arity(arities, {:when, _meta, [head | _guards]}),
    do: put_preload_arity(arities, head)

  defp put_preload_arity(arities, {:preload, _meta, args}) when is_list(args) do
    MapSet.put(arities, length(args))
  end

  defp put_preload_arity(arities, _head), do: arities

  defp maybe_report_pipe(ast, state) do
    stages = flatten_pipe(ast)

    stages
    |> truncations_by_stage()
    |> Enum.reduce(state, fn {truncation, truncation_index}, acc ->
      maybe_report_truncation(stages, truncation, truncation_index, acc)
    end)
  end

  defp maybe_report_truncation(stages, truncation, truncation_index, state) do
    with {:ok, field} <- preloaded_field_for_truncation(stages, truncation_index, state),
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

  defp preloaded_field_for_truncation(stages, truncation_index, state) do
    stages
    |> Enum.take(truncation_index)
    |> Enum.with_index()
    |> Enum.find_value(:error, fn {stage, index} ->
      stage
      |> unbounded_preload_fields(index > 0, state)
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
    pipeline_field_value_before_truncation?(stages, preload_index, truncation_index, field) or
      field_truncated_in_stage?(Enum.at(stages, truncation_index), field)
  end

  defp pipeline_field_value_before_truncation?(stages, preload_index, truncation_index, field) do
    stages
    |> Enum.slice(preload_index + 1, truncation_index - preload_index - 1)
    |> Enum.reduce_while(:source, fn stage, relation ->
      next_relation = field_value_relation(stage, relation, field)

      if next_relation == :other do
        {:halt, :other}
      else
        {:cont, next_relation}
      end
    end)
    |> Kernel.==(:field)
  end

  defp field_value_relation(stage, relation, field) do
    cond do
      extracts_field_value?(stage, field) -> :field
      relation == :field -> :field
      extracts_other_field_value?(stage, field) -> :other
      true -> relation
    end
  end

  defp field_truncated_in_stage?(stage, field) do
    {_ast, found?} =
      Macro.prewalk(stage, false, fn node, found? ->
        {node, found? or field_truncation?(node, field)}
      end)

    found?
  end

  defp unbounded_preload_fields(stage, piped?, state) do
    stage
    |> preload_specs(piped?, state)
    |> Enum.reject(fn {_field, bounded?} -> bounded? end)
    |> Enum.map(fn {field, _bounded?} -> field end)
  end

  defp preload_specs({{:., _, [module_ast, :preload]}, _meta, args}, piped?, state)
       when is_list(args) do
    cond do
      repo_module?(module_ast) ->
        args
        |> repo_preload_specs_arg(piped?)
        |> preload_specs_from_result()

      ecto_query_module?(module_ast, state) ->
        args
        |> ecto_preload_specs_arg(piped?, true)
        |> preload_specs_from_result()

      true ->
        []
    end
  end

  defp preload_specs({:preload, _meta, args}, piped?, state) when is_list(args) do
    if imported_ecto_preload?(args, piped?, state) do
      args
      |> ecto_preload_specs_arg(piped?, true)
      |> preload_specs_from_result()
    else
      []
    end
  end

  defp preload_specs({:from, _meta, args}, _piped?, _state) when is_list(args) do
    args
    |> Enum.flat_map(&from_preload_specs/1)
  end

  defp preload_specs(_stage, _piped?, _state), do: []

  defp repo_preload_specs_arg([specs], true), do: {:ok, specs}
  defp repo_preload_specs_arg([specs, _opts], true), do: {:ok, specs}
  defp repo_preload_specs_arg([_source, specs], false), do: {:ok, specs}
  defp repo_preload_specs_arg([_source, specs, _opts], false), do: {:ok, specs}
  defp repo_preload_specs_arg(_args, _piped?), do: :error

  defp preload_specs_from_result({:ok, specs}), do: preload_specs_for_arg(specs)
  defp preload_specs_from_result(:error), do: []

  defp imported_ecto_preload?(args, piped?, state) do
    state.ecto_query_imported? and
      not MapSet.member?(state.local_preload_arities, preload_call_arity(args, piped?))
  end

  defp preload_call_arity(args, true), do: length(args) + 1
  defp preload_call_arity(args, false), do: length(args)

  defp ecto_preload_specs_arg([specs], true, true), do: {:ok, specs}

  defp ecto_preload_specs_arg([_query_or_binding_ast, specs], _piped?, true), do: {:ok, specs}

  defp ecto_preload_specs_arg([_query_ast, binding_ast, specs], _piped?, true) do
    if ecto_binding_list?(binding_ast),
      do: {:ok, specs},
      else: :error
  end

  defp ecto_preload_specs_arg(_args, _piped?, _ecto_query_imported?), do: :error

  defp ecto_binding_list?(binding_ast) when is_list(binding_ast),
    do: not Keyword.keyword?(binding_ast)

  defp ecto_binding_list?(_binding_ast), do: false

  defp preload_specs_for_arg(field) when is_atom(field), do: [{field, false}]

  defp preload_specs_for_arg(fields) when is_list(fields) do
    Enum.flat_map(fields, fn
      {field, query_ast} when is_atom(field) ->
        [{field, bounded_preload_query?(query_ast)}]

      field when is_atom(field) ->
        [{field, false}]

      _ ->
        []
    end)
  end

  defp preload_specs_for_arg(_arg), do: []

  defp from_preload_specs(arg) when is_list(arg) do
    if Keyword.keyword?(arg) do
      arg
      |> Keyword.get(:preload)
      |> preload_specs_for_arg()
    else
      []
    end
  end

  defp from_preload_specs(_arg), do: []

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

  defp extracts_field_value?(stage, field) do
    case field_extraction(stage) do
      {:ok, key} -> field_key?(key, field)
      :error -> false
    end
  end

  defp extracts_other_field_value?(stage, field) do
    case field_extraction(stage) do
      {:ok, key} -> not field_key?(key, field)
      :error -> false
    end
  end

  defp field_extraction({{:., _, [module_ast, function]}, _meta, args})
       when function in [:fetch!, :get] and is_list(args) do
    if map_module?(module_ast), do: field_extraction_arg(args), else: :error
  end

  defp field_extraction({:then, _meta, [fun_ast]}), do: then_field_extraction(fun_ast)
  defp field_extraction(_stage), do: :error

  defp field_extraction_arg([key]), do: field_from_key(key)
  defp field_extraction_arg([_source, key]), do: field_from_key(key)
  defp field_extraction_arg(_args), do: :error

  defp field_from_key(key) when is_atom(key) or is_binary(key), do: {:ok, key}
  defp field_from_key(_key), do: :error

  defp then_field_extraction({:fn, _meta, [{:->, _arrow_meta, [_args, body]}]}) do
    body_field_extraction(body)
  end

  defp then_field_extraction({:&, _meta, [body]}), do: body_field_extraction(body)
  defp then_field_extraction(_fun_ast), do: :error

  defp body_field_extraction({{:., _, [_object_ast, field]}, _meta, []}) when is_atom(field),
    do: {:ok, field}

  defp body_field_extraction(_body), do: :error

  defp field_truncation?({:|>, _meta, [left, right]}, field),
    do: references_field?(left, field) and match?({:ok, _truncation}, truncation_info(right))

  defp field_truncation?({{:., _, [module_ast, function]}, _meta, args} = node, field)
       when is_list(args) do
    direct_truncation_on_field?(module_ast, function, args, field) or
      map_field_update_truncates?(node, field)
  end

  defp field_truncation?(_node, _field), do: false

  defp direct_truncation_on_field?(module_ast, function, [first_arg | args], field) do
    with {:ok, module} <- truncation_module(module_ast),
         true <- truncation_function?(module, function, length([first_arg | args])) do
      references_field?(first_arg, field)
    else
      _ -> false
    end
  end

  defp direct_truncation_on_field?(_module_ast, _function, _args, _field), do: false

  defp map_field_update_truncates?({{:., _, [module_ast, function]}, _meta, args}, field)
       when function in [:update, :update!] and is_list(args) do
    map_module?(module_ast) and
      Enum.any?(args, &field_key?(&1, field)) and
      match?({:ok, _truncation}, truncation_info(List.last(args)))
  end

  defp map_field_update_truncates?(_stage, _field), do: false

  defp map_module?({:__aliases__, _, [:Map]}), do: true
  defp map_module?(_module_ast), do: false

  defp field_key?(key, field), do: key == field or key == Atom.to_string(field)

  defp bounded_preload_query?(ast), do: contains_limit?(ast) or unresolved_query?(ast)

  defp contains_limit?({:from, _meta, args}) when is_list(args),
    do: Enum.any?(args, &from_limit_arg?/1)

  defp contains_limit?({:^, _meta, [ast]}), do: contains_limit?(ast)

  defp contains_limit?({{:., _, [module_ast, :limit]}, _meta, args}) when is_list(args),
    do: ecto_query_module?(module_ast) and length(args) in [1, 2]

  defp contains_limit?({:limit, _meta, args}) when is_list(args),
    do: length(args) in [1, 2]

  defp contains_limit?({:|>, _meta, [left, right]}),
    do: contains_limit?(left) or contains_limit?(right)

  defp contains_limit?(_ast), do: false

  defp from_limit_arg?(arg) when is_list(arg),
    do: Keyword.keyword?(arg) and Keyword.has_key?(arg, :limit)

  defp from_limit_arg?(_arg), do: false

  defp unresolved_query?({:^, _meta, [ast]}), do: unresolved_query?(ast)

  defp unresolved_query?({:|>, _meta, [left, _right]}), do: unresolved_query?(left)

  defp unresolved_query?({{:., _, [_object_ast, _field]}, _meta, []}), do: false

  defp unresolved_query?({{:., _, [module_ast, function]}, _meta, args})
       when is_list(args),
       do: unresolved_module_call?(module_ast, function, args)

  defp unresolved_query?({:from, _meta, _args}), do: false
  defp unresolved_query?({:%{}, _meta, _pairs}), do: false

  defp unresolved_query?({_name, _meta, args}) when is_list(args), do: true
  defp unresolved_query?({_name, _meta, context}) when is_atom(context), do: true
  defp unresolved_query?(_ast), do: false

  defp unresolved_module_call?(module_ast, :limit, _args) do
    not ecto_query_module?(module_ast)
  end

  defp unresolved_module_call?(module_ast, _function, [query_ast | _args]) do
    if ecto_query_module?(module_ast), do: unresolved_query?(query_ast), else: true
  end

  defp unresolved_module_call?(_module_ast, _function, _args), do: true

  defp ecto_query_module?(module_ast, state) do
    ecto_query_module?(module_ast) or aliased_ecto_query_module?(module_ast, state)
  end

  defp aliased_ecto_query_module?({:__aliases__, _, parts}, state),
    do: MapSet.member?(state.ecto_query_aliases, parts)

  defp aliased_ecto_query_module?(_module_ast, _state), do: false

  defp ecto_query_module?({:__aliases__, _, [:Ecto, :Query]}), do: true
  defp ecto_query_module?(_module_ast), do: false

  defp repo_module?({:__aliases__, _, parts}), do: List.last(parts) == :Repo
  defp repo_module?(_module_ast), do: false

  defp read_model_path?(filename) when is_binary(filename) do
    String.contains?(filename, "/lib/canary/query/") or
      String.starts_with?(filename, "lib/canary/query/") or
      String.ends_with?(filename, "/lib/canary/query.ex") or
      filename == "lib/canary/query.ex"
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
          "or split into explicit count and limited-fetch helpers. " <>
          "See `lib/canary/query/incidents.ex:fetch_top_signals/3` for the reference shape.",
      trigger: field_name,
      line_no: line_no
    )
  end

  defp add_issue(state, issue) do
    %{state | issues: [issue | state.issues]}
  end
end

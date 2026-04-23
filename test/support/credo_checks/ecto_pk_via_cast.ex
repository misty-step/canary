defmodule Canary.Checks.EctoPKViaCast do
  use Credo.Check,
    category: :warning,
    base_priority: :high,
    explanations: [
      check: """
      Canary schemas use custom string primary keys for externally meaningful IDs.
      Passing those keys through `changeset/2` is unsafe unless the schema casts
      the primary key field: Ecto silently drops fields absent from the cast list.

      Set custom primary keys on the struct before calling the changeset.
      """
    ]

  alias Credo.{IssueMeta, SourceFile}
  alias Credo.Execution.ExecutionIssues

  @doc false
  @impl true
  def run_on_all_source_files(exec, source_files, params) do
    schema_index = build_schema_index(source_files)

    Enum.each(source_files, fn source_file ->
      source_file
      |> issues_for_source_file(params, schema_index)
      |> then(&ExecutionIssues.append(exec, &1))
    end)

    :ok
  end

  @doc false
  @impl true
  def run(%SourceFile{} = _source_file, _params), do: []

  defp issues_for_source_file(source_file, params, schema_index) do
    source_file
    |> Credo.Code.prewalk(&walk/2, initial_state(source_file, params, schema_index))
    |> Map.fetch!(:issues)
    |> Enum.reverse()
  end

  defp initial_state(source_file, params, schema_index) do
    %{
      aliases: %{},
      issue_meta: IssueMeta.for(source_file, params),
      issues: [],
      map_bindings: %{},
      schema_index: schema_index
    }
  end

  defp walk({:alias, _, _} = ast, state) do
    {ast, update_aliases(state, ast)}
  end

  defp walk({:=, _, [{name, _, context}, value]} = ast, state)
       when is_atom(name) and (is_atom(context) or is_nil(context)) do
    key_names = attr_key_names(value, state.map_bindings)

    state =
      if key_names == [] do
        state
      else
        put_in(state.map_bindings[name], key_names)
      end

    {ast, state}
  end

  defp walk({:|>, _, _} = ast, state) do
    {ast, maybe_report_pipe(ast, state)}
  end

  defp walk(ast, state), do: {ast, state}

  defp maybe_report_pipe(ast, state) do
    with {:ok, struct_module_ast, struct_fields} <- struct_literal(ast),
         {:ok, changeset_module_ast, attrs_ast, line_no} <- changeset_call(ast),
         {:ok, schema} <-
           resolve_same_module(struct_module_ast, changeset_module_ast, state.aliases),
         {:ok, schema_metadata} <- schema_metadata(schema, state.schema_index),
         primary_key_name = schema_metadata.primary_key,
         false <- primary_key_name in key_names(struct_fields),
         false <- schema_metadata.casts_primary_key?,
         true <- primary_key_name in attr_key_names(attrs_ast, state.map_bindings) do
      add_issue(state, issue_for(state.issue_meta, schema, primary_key_name, line_no))
    else
      _ -> state
    end
  end

  defp build_schema_index(source_files) do
    Enum.reduce(source_files, %{}, fn source_file, schema_index ->
      source_file
      |> module_definitions()
      |> Enum.reduce(schema_index, fn {module_ast, body}, acc ->
        case extract_schema_metadata(module_ast, body) do
          %{module: module} = metadata -> Map.put(acc, module, metadata)
          nil -> acc
        end
      end)
    end)
  end

  defp module_definitions(source_file) do
    source_file
    |> Credo.Code.prewalk(
      fn
        {:defmodule, _, [module_ast, [do: body]]} = ast, modules ->
          {ast, [{module_ast, body} | modules]}

        ast, modules ->
          {ast, modules}
      end,
      []
    )
  end

  defp extract_schema_metadata(module_ast, body) do
    module = resolve_module(module_ast, %{})

    {_, state} =
      Macro.prewalk(
        body,
        %{attrs: %{}, casts: MapSet.new(), has_schema?: false, primary_key: nil},
        fn
          {:schema, _, _} = ast, state ->
            {ast, %{state | has_schema?: true}}

          {:@, _, [{:primary_key, _, [value_ast]}]} = ast, state ->
            {ast, %{state | primary_key: primary_key_metadata(value_ast)}}

          {:@, _, [{name, _, [value_ast]}]} = ast, state when is_atom(name) ->
            value = eval_module_attr(value_ast, state.attrs)
            attrs = if value == nil, do: state.attrs, else: Map.put(state.attrs, name, value)
            {ast, %{state | attrs: attrs}}

          {:cast, _, args} = ast, state ->
            {ast, collect_cast_fields(state, args)}

          ast, state ->
            {ast, state}
        end
      )

    case state do
      %{has_schema?: true, primary_key: %{type: :string, autogenerate: false, field: field}} ->
        %{
          module: module,
          primary_key: Atom.to_string(field),
          casts_primary_key?: MapSet.member?(state.casts, Atom.to_string(field))
        }

      _ ->
        nil
    end
  end

  defp primary_key_metadata({:{}, _, [field, type, opts]})
       when is_atom(field) and is_atom(type) and is_list(opts) do
    %{autogenerate: Keyword.get(opts, :autogenerate, true), field: field, type: type}
  end

  defp primary_key_metadata(_), do: nil

  defp collect_cast_fields(state, [_attrs_ast, fields_ast]) do
    update_in(
      state.casts,
      &MapSet.union(&1, MapSet.new(eval_field_names(fields_ast, state.attrs)))
    )
  end

  defp collect_cast_fields(state, [_data_ast, _attrs_ast, fields_ast]) do
    update_in(
      state.casts,
      &MapSet.union(&1, MapSet.new(eval_field_names(fields_ast, state.attrs)))
    )
  end

  defp collect_cast_fields(state, _args), do: state

  defp eval_module_attr({:sigil_w, _, [{:<<>>, _, [words]}, ~c"a"]}, _attrs)
       when is_binary(words) do
    words
    |> String.split()
    |> Enum.map(&String.to_atom/1)
  end

  defp eval_module_attr(fields, _attrs) when is_list(fields) do
    if Enum.all?(fields, &(is_atom(&1) or is_binary(&1))) do
      fields
    else
      nil
    end
  end

  defp eval_module_attr(_value_ast, _attrs), do: nil

  defp eval_field_names({:@, _, [{name, _, nil}]}, attrs),
    do: List.wrap(attrs[name]) |> Enum.map(&to_string/1)

  defp eval_field_names({:++, _, [left, right]}, attrs) do
    (eval_field_names(left, attrs) ++ eval_field_names(right, attrs))
    |> Enum.uniq()
  end

  defp eval_field_names(fields, _attrs) when is_list(fields) do
    Enum.map(fields, &to_string/1)
  end

  defp eval_field_names(_fields_ast, _attrs), do: []

  defp schema_metadata(schema, schema_index) do
    case Map.get(schema_index, schema) do
      nil -> compiled_schema_metadata(schema)
      metadata -> {:ok, metadata}
    end
  end

  defp compiled_schema_metadata(schema) when is_atom(schema) do
    if Code.ensure_loaded?(schema) and function_exported?(schema, :__schema__, 1) do
      case schema.__schema__(:primary_key) do
        [primary_key] ->
          if schema.__schema__(:autogenerate_id) == nil and
               schema.__schema__(:type, primary_key) == :string do
            {:ok,
             %{
               casts_primary_key?: changeset_casts_primary_key?(schema, primary_key),
               module: schema,
               primary_key: Atom.to_string(primary_key)
             }}
          else
            :error
          end

        _ ->
          :error
      end
    else
      :error
    end
  rescue
    _ -> :error
  end

  defp compiled_schema_metadata(_), do: :error

  defp changeset_casts_primary_key?(schema, primary_key) do
    changeset =
      apply(schema, :changeset, [struct(schema), %{primary_key => "canary-credo-probe"}])

    Map.has_key?(changeset.changes, primary_key)
  rescue
    _ -> false
  end

  defp struct_literal({:|>, _, [struct_ast, _]}), do: struct_literal(struct_ast)

  defp struct_literal({:%, _, [module_ast, {:%{}, _, fields}]}) do
    {:ok, module_ast, fields}
  end

  defp struct_literal(_), do: :error

  defp changeset_call({:|>, meta, [_, {{:., _, [module_ast, :changeset]}, _, [attrs_ast]}]}) do
    {:ok, module_ast, attrs_ast, meta[:line]}
  end

  defp changeset_call(_), do: :error

  defp resolve_same_module(left_ast, right_ast, aliases) do
    left = resolve_module(left_ast, aliases)
    right = resolve_module(right_ast, aliases)

    if left == right and is_atom(left) do
      {:ok, left}
    else
      :error
    end
  end

  defp resolve_module({:__aliases__, _, [name]}, aliases) do
    Map.get(aliases, name, Module.concat([name]))
  end

  defp resolve_module({:__aliases__, _, parts}, _aliases) do
    Module.concat(parts)
  end

  defp resolve_module(_, _aliases), do: nil

  defp update_aliases(state, {:alias, _, [{{:., _, [base_ast, :{}]}, _, children}]}) do
    base_parts = module_parts(base_ast)

    aliases =
      Enum.reduce(children, state.aliases, fn child, aliases ->
        parts = module_parts(child)
        Map.put(aliases, List.last(parts), Module.concat(base_parts ++ parts))
      end)

    %{state | aliases: aliases}
  end

  defp update_aliases(state, {:alias, _, [module_ast, [as: {:__aliases__, _, [as_name]}]]}) do
    put_in(state.aliases[as_name], module_ast |> module_parts() |> Module.concat())
  end

  defp update_aliases(state, {:alias, _, [module_ast]}) do
    parts = module_parts(module_ast)
    put_in(state.aliases[List.last(parts)], Module.concat(parts))
  end

  defp update_aliases(state, _), do: state

  defp module_parts({:__aliases__, _, parts}), do: parts

  defp attr_key_names({:%{}, _, pairs}, _bindings), do: key_names(pairs)

  defp attr_key_names({name, _, context}, bindings)
       when is_atom(name) and (is_atom(context) or is_nil(context)) do
    Map.get(bindings, name, [])
  end

  defp attr_key_names({{:., _, [map_module, :merge]}, _, [left, right]}, bindings) do
    if map_module?(map_module) do
      (attr_key_names(left, bindings) ++ attr_key_names(right, bindings))
      |> Enum.uniq()
    else
      []
    end
  end

  defp attr_key_names({{:., _, [map_module, :put]}, _, [map_ast, key_ast, _value]}, bindings) do
    if map_module?(map_module) do
      (attr_key_names(map_ast, bindings) ++ key_names([{key_ast, nil}]))
      |> Enum.uniq()
    else
      []
    end
  end

  defp attr_key_names(_, _bindings), do: []

  defp map_module?({:__aliases__, _, [:Map]}), do: true
  defp map_module?(_), do: false

  defp key_names(pairs) when is_list(pairs) do
    pairs
    |> Enum.flat_map(&key_name/1)
    |> Enum.uniq()
  end

  defp key_name({key, _value}) when is_atom(key), do: [Atom.to_string(key)]
  defp key_name({key, _value}) when is_binary(key), do: [key]
  defp key_name(_), do: []

  defp issue_for(issue_meta, schema, primary_key_name, line_no) do
    schema_name = inspect(schema)

    format_issue(
      issue_meta,
      message:
        "Custom primary key `:#{primary_key_name}` is being passed through #{schema_name}.changeset/2. " <>
          "The schema does not cast `:#{primary_key_name}`, so Ecto will silently drop it " <>
          "(CLAUDE.md footgun #1). Set it on the struct instead: " <>
          "%#{schema_name}{#{primary_key_name}: #{primary_key_name}} |> #{schema_name}.changeset(attrs_without_#{primary_key_name}).",
      trigger: primary_key_name,
      line_no: line_no
    )
  end

  defp add_issue(state, issue) do
    %{state | issues: [issue | state.issues]}
  end
end

# AIR Condition DSL

AIR conditions are intentionally small. They are routing guards for bounded state machines and RunPlan nodes, not a general expression language.

## Grammar

```text
condition  := or_group ("||" or_group)*
or_group   := clause ("&&" clause)*
clause     := path ("==" | "!=") literal
path       := identifier ("." identifier)*
literal    := quoted_string | json_literal
```

`&&` binds tighter than `||` because AIR evaluates a condition as OR groups of AND clauses. Parentheses are not supported.

Examples:

```yaml
when: phase == "route" && decision.complete == false
when: supervisor.decision.action == "conduct_research" || supervisor.decision.needs_more == true
```

## Paths

State-machine conditions read from the merged state/output namespace:

- `phase`
- `decision.complete`
- `state.phase`
- `output.result`
- `outputs.result`

RunPlan node conditions read endpoint paths:

- `$input.question`
- `planner.plan.needs_more`
- `supervisor.decision.action`

Append endpoints such as `notes[]` are not valid in conditions.

## Literals

Quoted strings are supported with single or double quotes:

```yaml
when: phase == "done"
when: route != 'fallback'
```

Unquoted literals are parsed as JSON:

```yaml
when: approved == true
when: retry_count == 2
when: result == null
```

## Limits

The current DSL is deliberately conservative:

- no parentheses;
- no numeric comparison operators such as `<`, `<=`, `>`, or `>=`;
- no arithmetic, functions, regexes, `in`, or `contains`;
- no escaping-aware tokenization, so string literals must not contain `&&`, `||`, `==`, or `!=`;
- quoted strings are treated as simple delimiters, not full JSON string decoding.

Use a module action to derive a boolean or enum field, then route on that field, when logic grows beyond equality checks.

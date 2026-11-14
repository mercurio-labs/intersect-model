# Defining and executing Rcore models

Rcore is the authored definition language for Intersect Model. A source can
describe structure and supported behavior. The direct API compiles those sources
without an Ecore XML round trip, a text grammar, or Java.

This is the maintained element reference. Syntax acceptance, type checking and
execution are separate contracts: the tables describe authored elements, while
[execution paths](#choose-the-execution-path) identify which service activates
them. The archived milestone documents are historical evidence, not a second
current language specification.

- [First source](#start-with-a-source) and [lexical rules](#lexical-rules)
- [Structure](#structural-elements), [types](#structural-types-and-metadata)
  and [behavior declarations](#behavior-declarations)
- [Behavior types](#behavior-types), [expressions](#expressions-and-patterns)
  and [builtins](#builtins-and-prelude)
- [Dependencies](#supply-dependencies-explicitly),
  [execution](#choose-the-execution-path) and [limits](#limits-and-compatibility)

## Start with a source

```text
rcore 1;
package example version "1";
metamodel "urn:example" prefix example {
    class Item { attr name: String [1,1]; }
}
```

`compile_rcore(text)` uses a default in-memory source URI.
`compile_rcore_at(uri, text)` retains the URI supplied by the application.
The [README](../README.md#a-small-model) contains a complete executable Rust
consumer. `RcorePackage::new_model` produces a checked model; create/set/validate
and save/reload use the shared model services.

The README first builds the same schema with `SchemaBuilder`, then with Rcore.
Run both with `cargo run -p intersect-model --example schema_choices --locked`.
Rcore is useful when definitions should be authored, retained and recompiled as
source; a dynamic schema is useful when the application constructs definitions.

## Lexical rules

Every source starts with `rcore 1; package qualified.name version "1";`.
The syntax version (`1`), application package version (the string) and internal
execution IR version are distinct. Historical documents called the typed frontend
“v2”; that does **not** mean writing `rcore 2;`.

Names use ASCII letters, digits and underscores, starting with a letter or
underscore. Backticks quote model names, including reserved words and Unicode;
double a backtick inside a quoted name. Empty names and line breaks reject.
Behavior paths use dots (`m.Node`); nested structural type paths use `::`
(`nested::Node`). Identifiers beginning with `#` are reserved for parser expansions.

Whitespace, `//` comments and nested `/* ... */` comments separate tokens.
Strings use double quotes and JSON escapes. Numbers retain decimal spelling,
including fractions and signed exponents; a leading minus is an operator.
Radix literals and numeric suffixes are not supported. Diagnostics use half-open
UTF-8 byte spans in the exact retained source, not character positions.

## Structural elements

A `metamodel "namespace" prefix shortName { ... }` contains declarations.
For multiple roots use `resource { package name namespace "uri" prefix p { ... } }`
after the source header. Select the desired namespace explicitly when compiling
a multi-root source. Nested packages use that same `package ... namespace ...`
form inside a metamodel/package body.

| Element | Form and meaning |
| --- | --- |
| Class | `class Item { ... }`; owns features and operation signatures |
| Abstract class | `abstract class Named { ... }`; cannot be instantiated directly |
| Interface | `interface class Named { ... }`; records interface semantics |
| Inheritance | `class Item extends Named, Tagged { ... }`; shared resolution checks inheritance and feature conflicts |
| Attribute | `attr name: String [1,1];`; stores a datatype or enum value |
| Reference | `ref children: Item [0,*] containment;`; refers to model objects |
| Operation | `op label(text: String [1,1]) -> String [1,1];`; signature only, implementation must be bound |
| Void operation | `op clear() -> void;`; no result slot |
| Exception metadata | `op read() -> String [0,1] throws Failure;`; retained declared types, not an installed exception handler |
| Enum | `enum State { OPEN = 0; CLOSED = 1 literal "closed"; }`; named signed 32-bit values with optional wire spelling |
| Datatype | `datatype Token instanceClass "java.lang.String" {}`; nominal type whose native codec/provider must be supported |
| Annotation | `annotation "documentation" { detail "text" = "An item"; }`; retained metadata, not executable code |

The class/interface spelling and modifiers are implemented by the
[structural parser](../crates/intersect-model/src/definitions/rcore.rs).
An empty enum has no possible value. Enum names must be distinct; their numeric
values may alias. Empty packages can be represented but do not alone supply an
executable model schema.

### Multiplicity and feature modifiers

Every feature and non-void operation slot has `[lower,upper]`: `[1,1]` required,
`[0,1]` optional, `[0,*]` many. Bounds are nonnegative, lower cannot exceed upper,
and `*` means unbounded. Ordered and unique are the defaults.

| Modifier | Contract |
| --- | --- |
| `unordered`, `nonunique` | Change collection ordering/uniqueness; also available on operation slots |
| `containment` | Reference owns its children; shared checks enforce containment rules |
| `opposite Owner.feature` | Pair two reference ends; an external opposite can use `opposite external "uri#//Owner/feature"` |
| `default "text"`, `default 1`, `default true` | Attribute literal interpreted by its declared codec |
| `default State.CLOSED` | Checked enum default; it must name the declared enum and an existing literal |
| `derived` | Value requires a derived implementation; cannot be assumed to be ordinary stored data |
| `readonly` | Records nonchangeable feature semantics |
| `transient`, `volatile` | Preserve persistence/storage metadata; each execution/interchange path determines admission |
| `unsettable` | Preserve explicit assignment-state semantics where supported |
| `id` | Mark an identifying attribute |
| `noresolve` | Preserve reference proxy-resolution metadata |

Defaults are attribute-only; opposites and containment are reference-only.
Repeated/unknown modifiers and unsupported combinations diagnose. A parsed flag
does not establish that every storage or export path supports its semantics.
In particular, an imported delegate annotation never installs an implementation.

## Structural types and metadata

Structural aliases are `String`, `Bool`, `Int32`, `Int64`, `Float32` and `Float64`;
explicit Ecore builtin names are also available. Other names resolve to declared
types. Quote a local type that collides with a builtin, for example
`` attr choice: `EInt` [0,1]; ``. Behavior has a larger scalar vocabulary below.

| Form | Meaning |
| --- | --- |
| `class Box<T> { ref value: T [0,1]; }` | Generic structural declaration |
| `T extends Base & Tagged` | Multiple type-parameter bounds |
| `Box<Item>` | Explicit application; runtime selection must be closed |
| `Box<?>`, `Box<? extends Base>`, `Box<? super Item>` | Wildcard arguments; declaration support is separate from runtime materialization |
| `generic Box<Item>` | Explicit generic type representation for retained declarations |
| `parameter "T"` | Explicit type-parameter reference representation |
| `external "memory:types.rcore#//Item"` | Cross-resource type; requires the exact supplied dependency |
| `op copy<T>(value: T [1,1]) -> T [1,1];` | Generic operation signature; does not install generic execution |
| `identity "..."`, `instanceClass "..."`, `instanceType "..."` | Classifier metadata; Java spellings do not load Java classes |
| `nonserializable` | Datatype/enum serialization metadata |

Classifier metadata follows the classifier name/type parameters; datatype/enum
`nonserializable` follows that metadata. Annotations can carry `detail "key" =
"value";`, `reference "uri#fragment";` and nested annotations. Package, classifier,
feature, enum literal, type parameter and operation metadata preserve provenance.
Source-authored `identity` metadata is not a replacement for a source digest.

Use `compile_rcore_bundle_selected` for namespace and exact `RuntimeType`
selection, and `create_typed` for explicit generic applications. Same-name types
from different source revisions or generic applications are not interchangeable.
See the [structural extension controls](../crates/intersect-model/tests/rcore_api.rs)
and [syntax implementation](../crates/intersect-model/src/definitions/rcore_structure_syntax.rs).

## Behavior declarations

Put behavior after the metamodel in `behavior { ... }`, or supply a standalone
behavior source with its own `rcore 1; package ... version ...;` header.
Imports select exact supplied sources; they never fetch content. `pub` exports a
function/derive/validate declaration to importing modules; omitted visibility or
`private` keeps it private. Import aliases are not implicitly re-exported.

| Element | Form and purpose |
| --- | --- |
| Model import | `import model "urn:example" as m;` selects model classifier identities |
| Module import | `import package helpers version "1" as h;` selects a declared behavior dependency |
| Function | `fn label(x: String) -> String = x;`; a body can also be a result-producing block |
| Public/generic function | `pub fn identity<T>(x: T) -> T = x;`; constraints use `T: Constraint` where admitted by the checker |
| External function | `extern fn lookup(name: String) -> String;`; source signature requiring an explicitly registered provider |
| Derived expression | `derive m.Item.label(self) -> String = self.name;`; checked against a derived feature's contract |
| Validation rule | `validate m.Item.named(self) { require self.name != "" code "name.empty" message "Name required"; }` |
| Builtin derivation | `derive m.Group.size using count(children);` or `derive m.Node.root using noIncoming(m.Node.links);` |
| Projection | `projection m.Item.displayName from name writable;`; derived feature backed by a stored feature with the same contract; `writable` is optional |
| Initializer | `initialize m.Item using "provider" version "1" { name = "new"; }`; explicit host contract with constant member values; a member may also be listed without a value |
| Grammar conversion | `convert "context" rule "RULE" from "grammar-uri" sha256 "digest" span 0 10 to "datatype" using "provider" version "1" { "yes" = true; }`; exact grammar declaration and Boolean keyword map |
| Scope binding | `bind scope m.Node.links = resolve;`; choose the function used to resolve reference spellings |
| Default binding | `bind default m.Node.links = defaults;`; supply a stored reference left empty/unassigned |
| Implicit binding | `bind implicit m.Node.children = children;`; construct implicit objects for containment |
| Validation binding | `bind validate m.Node = check;`; select a function returning validation issues |

These are syntax forms, not a single facade's activation promise. The grammar
conversion line is a template: replace its URI, digest, span and provider with
real matching identities. External declarations, initializers and conversions
require matching host contracts. Direct `RcorePackage` supports admitted
derive/validate bodies in the selected entry: call `evaluate_model_rules` or
`read_derived` explicitly. Editing does not automatically reevaluate or invalidate
these results. Imported model rules, projections, grammar conversions and builtin
derived contracts are refused by this facade. Its selected-entry initializer
supports `intersect.constant-attributes@1`; other native initializers require the
explicit initializer compilation APIs. Frozen bindings use their owning engine.

The four `bind` forms have distinct engine contracts:

| Binding | Function result and execution point |
| --- | --- |
| scope | `(Owner, String) -> Sequence<Target>` while linking |
| implicit | `(Owner) -> Sequence<Child>`; admits `new`, before defaults |
| default | `(Owner) -> Sequence<Target>` after linking for an empty slot |
| validate | `(Owner) -> Sequence<Issue>` after structural validation; read-only |

`Issue` expands to a record with String fields `severity`, `code`, `message`,
`feature`, `rule`. `issue`, `issue_at` and `issue_rule` populate three, four or
five fields respectively; omitted fields are empty strings. Severity is `error`,
`warning` or `info`. A failed validator becomes an `internal.validation` error
issue, including budget exhaustion. Several validation bindings can target the
same class and apply to its subclasses.

## Behavior types

| Family | Types and rules |
| --- | --- |
| Scalars | `Bool`, `String`, `Bytes`, `Char16`, `DateInstant`, `Unit`; scalar codecs retain exact domain information |
| Integers | `Int` aliases `Int64`; signed `Int8/16/32/64`, unsigned `UInt8/16/32/64`, and `BigInt` |
| Other numbers | `Decimal`, `Float32`, `Float64`; no implicit numeric promotion |
| Model types | `m.Item` identifies the exact declaration; subtype assignment follows the shared inheritance graph |
| Optional | `Option<T>` or `T?`; `Some(value)`, `None`/`none`; no implicit lift or unwrap |
| Result | `Result<T,E>`, `Ok(value)`, `Err(error)`; failure is distinct from optional absence |
| Collections | `Sequence<T>` ordered/repeated; `OrderedSet<T>` ordered/unique; `Set<T>` unordered/unique; `Bag<T>` unordered/repeated |
| Feature value | `m.Item [0,*] nonunique`; retains bounds, order and uniqueness |
| Tuple | `(String, Int)`; expression `(name, 1)`; one-element tuple uses a trailing comma in expressions |
| Record | `{name: String, count: Int}`; field names and types are structural |
| Function | `(Int, Int) -> Int`; lambdas capture lexical values |
| Generic | Named function parameters and explicit call arguments such as `identity::<String>("a")` |

Literal types use their checked context. Fixed integers reject fractions and
overflow; decimal/large integer analysis preserves exact values. Float admission
rejects nonfinite results and nonzero literals that underflow to zero. An empty
collection, `None`, or one-sided `Result` may require an annotation or explicit
type arguments. Optional, collection and model types cannot silently substitute
for one another. Downcasts use checked `asType::<T>` and return an `Option`.

## Expressions and patterns

| Form | Example / rule |
| --- | --- |
| Literal/name/group | `42`, `"text"`, `true`, `none`, `x`, `(x + 1)` |
| List/tuple/record | `[1, 2]`, `(1, "a")`, `{name: "a", count: 1}`; `()` is Unit |
| Member access | `node.name`; `node?.name` requires an optional receiver |
| Calls | `f(x)`, `h.f(x)`, `f::<Int>(x)` |
| Method notation | `xs.filter(predicate)` expands to `filter(xs, predicate)`; imported module calls stay qualified |
| Index | `xs[index]`; checked access, not unchecked native memory access |
| Unary | `!condition`, `-value`, `+value`, subject to operand types |
| Arithmetic | `+`, `-`, `*`, `/`, `%`, with scalar-domain checks |
| Comparisons | `<`, `<=`, `>`, `>=`, `==`, `!=`; unsupported equality domains diagnose |
| Boolean/coalesce | `&&`, `||` short circuit; `optional ?? fallback` handles absence |
| Conditional | `if condition { a } else { b }`; both branches have compatible result types |
| Block | `{ let x: Int = 1; x + 1 }`; annotation optional; final expression is the result |
| Lambda | `|x: Int| x + 1`; annotation may be inferred from checked context; `|| value` has no parameters |
| Comprehension | `[for x in xs, y in ys where predicate(x, y) yield f(x, y)]`; later generators see earlier bindings |
| Match | `match x { Some(v) => v, None => 0 }`; patterns may nest, bind names, match literals or use `_` |
| Type switch | `match node { x: m.Special => f(x), other => g(other) }`; ordered kind tests require a final catch-all |
| Construct | `new m.Item { name: "new" }`; only constructing execution paths admit object creation |

Precedence from low to high: `??`, `||`, `&&`, equality, comparisons, additive,
multiplicative, unary, postfix. Coalesce associates right; other binary operators
associate left. Parentheses override precedence. `match` checks exhaustiveness
and unreachable arms; open scalar/model domains need a catch-all. Type-switch
arms cannot be mixed with constructor/literal patterns.

Effects are inferred and verified: pure computation, model queries and
construction are distinct. A query cannot hide construction behind a helper call.
There is no general imperative mutation or I/O statement in these expressions;
checked host adapters and model transactions supply those capabilities explicitly.

## Builtins and prelude

The following is the current named builtin inventory, grouped by purpose. Exact
argument/result admission is checked by the shared type checker and execution
path. Collection kind and model declaration identities remain significant.

| Operations | Purpose |
| --- | --- |
| `map`, `filter`, `flatMap`, `fold` | Transform, select, flatten-map and accumulate |
| `exists`, `forAll`, `one`, `find` | Quantify or find; `find` returns an Option |
| `closure` | Follow a successor function with unique visited values |
| `sortedBy`, `sortedWith`, `groupBy` | Checked ordering and grouping |
| `union`, `intersection`, `difference`, `concat`, `distinct` | Collection algebra; result-kind rules are checked |
| `contains`, `excludes`, `count`, `includesAll`, `includesAny` | Membership and occurrence queries |
| `flatten`, `first`, `last`, `at`, `size`, `isEmpty` | Collection inspection; positional operations use zero-based indexes |
| `Sequence`, `OrderedSet`, `Set`, `Bag` | Explicit collection-kind conversions |
| `Some`, `None`, `Ok`, `Err` | Optional/result constructors |
| `incoming` | Inverse-reference query using the selected model feature |
| `container`, `contents`, `ancestors`, `roots` | Containment/model navigation |
| `asType` | Checked model-kind selection |
| `identity`, `resource` | Object identity and resource URI |
| `split`, `hex` | String splitting; lowercase hex of UTF-8 bytes |

Parser prelude forms add `selectByKind`, `oclIsKindOf`, `reject`, `notEmpty`,
`excluding`, type-switch matching and the Issue helpers. They expand to the same
checked primitives. `selectByKind` returns a Sequence; wrap it in `OrderedSet`
when a unique feature result is required. These names do not imply an OCL parser
or full OCL library. Source declarations can shadow prelude operations; using a
prelude name in a non-call position disables that expansion for the document.
Safe method calls (`x?.f()`) do not use ordinary method expansion.

See the [builtin inventory](../crates/intersect-model/src/definitions/rcore_behavior_bindings.rs),
[prelude implementation](../crates/intersect-model/src/definitions/rcore_behavior_parser_sugar.rs)
and [live-model prelude controls](../crates/intersect-model/tests/rcore_prelude.rs).

## Supply dependencies explicitly

Bundle APIs take a host-supplied source graph. A dependency includes its source
identity and exact content; the compiler does not resolve arbitrary filesystem
or network locations. A matching namespace is not a replacement for the expected
source revision. Keep source text, URI, digest, selected entry and dependency
edges together when caching or transporting definitions.

Use Ecore migration when importing existing metamodels. Inspect the conversion
report and retain original provenance. Unsupported constructs need an explicit
resolution; successful parsing or extraction does not establish native behavior.

## Choose the execution path

| Path | Contract |
| --- | --- |
| Direct Rcore package | Compile supported definitions, explicitly bind operations and invoke; evaluate admitted selected-entry model rules explicitly under limits |
| Frozen-engine bindings | Language-owned scope/default/implicit/validation bindings through their checked source contracts |
| Ordinary typed providers | Supported lowering/activation shapes; not every type-correct declaration can execute here |
| Native/Rhai adapters | Explicit source-authorized provider selection; declaration alone does not install an implementation |

The direct example binds `Node.reachable` to a checked function, executes it,
saves and reloads, then compares the result:

```sh
cargo run -p intersect-model --example rcore_model --locked
```

Read the [example source](../crates/intersect-model/examples/rcore_model.rs)
alongside its [Rcore input](../crates/intersect-model/assets/fixtures/rcore/api/organization.rcore).
Missing operation implementations, incompatible signatures and unsupported
activation return errors. Structural `model.validate()` does not run domain
rules that the host has not installed.

The [direct API controls](../crates/intersect-model/tests/rcore_api.rs) exercise
execution and reload, wrong bindings/arguments without partial edits, corrupt
saved data, explicit dependency identity and unsupported adapter refusal.
Frozen default/implicit/validation controls run separately through their owning
APIs. Ordinary typed lowering still rejects frozen-only `bind default`,
`bind implicit` and `bind validate`; a type-correct declaration does not install
those bindings. The [release plan](release-plan.md) requires these controls to be qualified
on the final source pair.

## Limits and compatibility

Hosts select execution budgets and own cancellation, memory admission and I/O.
Keep those responsibilities separate from deterministic language semantics.
Compiler identities are cache keys for exact compilation inputs, not signatures
or guarantees of compatibility with all earlier versions. Changed source,
features, dependency context or identity recipes may require rebuilding prepared
output.

The [release plan](release-plan.md) requires a path-specific support matrix and
negative cases before a release promise is frozen. Use the
[model guide](model-guide.md) for editing, persistence and provider contracts.

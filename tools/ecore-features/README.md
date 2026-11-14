# Independent Ecore feature-state observations

`Observe.java` is a verification-only host, never a build or production dependency.
It observes 32 independent combinations of changeable/derived/volatile/transient/
unsettable over scalar and many-valued dynamic EString features (64 cases).
Each operation records success/rejection, effective value and eIsSet. Input policy
is authored independently of Intersect; no native output generates the expected data.

Use a JDK supporting source-file launching (the recorded run used Corretto 17.0.10)
and set `EMF_CLASSPATH` to these exact JARs, separated by `;` on Windows or `:` on Unix:

- `org.eclipse.emf:org.eclipse.emf.ecore:2.38.0`
- `org.eclipse.emf:org.eclipse.emf.common:2.42.0`

Archive digests and the exact JDK/source/output identities are retained in
`fixtures/ecore-feature-flags/provenance.json`. Ecore's archive digest matches the
existing upstream pin; common 2.42.0 satisfies Ecore's declared [2.40.0,3.0.0) range.
No dependency is installed by this tool. From the repository root in PowerShell:

```powershell
java -cp $env:EMF_CLASSPATH tools/ecore-features/Observe.java target/e09-oracle/observations.json
python -B -m unittest discover -s tools -p 'test_ecore_feature_oracle.py'
```

Create the output directory before running. Review new observations and update
source/output provenance deliberately before replacing the committed cache. The
normal offline checks require Python only; they verify hashes, matrix coverage and
observed state distinctions without a Java installation or Maven cache.

Scope is dynamic EString attribute state, not generated getters, setting delegates,
references, all datatype families, copying or persistence. In this fixture,
changeable=false blocks reflective set/unset but not mutation through eGet's list;
derived/volatile/transient alone do not install a computed getter. These results
must not erase Mercurio's explicit constructor/delegate dependencies or become a
claim of full Ecore/SysML support. See `docs/ecore-e09-inventory.md`.


## Unsettable XMI save/load oracle

`ObserveXmi.java` independently constructs seven unsettable EString/reference
features and checks 17 save/load cases: omitted defaults, explicit defaults/nulls,
empty strings, empty collections, single reference targets and containment. It
writes an Ecore schema and unmodified XMI fixtures under the requested directory.
The schema uses its own relative URI; native controls compile it through an exact
resource-aware bundle rather than rewriting its reference spellings.

The recorded classpath uses common 2.42.0, Ecore 2.38.0 and XMI 2.37.0; XMI's
[2.36.0,3.0.0) Ecore range includes the pinned Ecore. Exact hashes and the source/JDK
identities are in `fixtures/ecore-unsettable/emf-xmi/provenance.json`. Java is only
an optional verification oracle, never a normal build or production dependency.

```powershell
java -cp $env:EMF_CLASSPATH tools/ecore-features/ObserveXmi.java target/e09-xmi-oracle
cargo run -p intersect-xmi --example unsettable_xmi_oracle --locked -- target/e09-xmi-native
java -cp $env:EMF_CLASSPATH tools/ecore-features/ObserveXmi.java target/e09-xmi-oracle target/e09-xmi-native
python -B -m unittest discover -s tools -p 'test_ecore_xmi_state_oracle.py'
```

The second Java run checks every native-exported case with EMF after normalizing
only resource-local object ID spelling. It compares all seven effective values and
assignment flags. Its cached report and input hashes record the checked run; the
Rust tests load every unchanged EMF fixture and verify native round trips without
Java. This is evidence for these 17 cases, not general XMI/datatype/opposite support.


## Inverse-reference state oracle

`ObserveOpposites.java` uses the original common/Ecore classpath and records 12
operations for each of 24 valid cardinality/containment/unsettable configurations.
The containment inverse is singular; non-containment includes all four cardinality
pairs. Both unsettable flags vary independently. Four stable object IDs retain
values, eIsSet and container identities after each operation, including replacement,
explicit empty/default, reset, inverse-side edits and owner transfer.

```powershell
java -cp $env:EMF_CLASSPATH tools/ecore-features/ObserveOpposites.java target/e09-opposites-observations.json
python -B -m unittest discover -s tools -p 'test_ecore_opposite_oracle.py'
```

Exact provenance is in `fixtures/ecore-feature-flags/opposites-provenance.json`.
The E09 baseline compared 240 applicable snapshots, explicitly rejecting the
remaining singular-inverse transfers. The E10 displacement test now compares all
288 snapshots (12 steps for all 24 configurations) through shared mutation/state
services. It includes the previously excluded 48 transfer/follow-up states; this
is new native consumption of the unchanged cache, not a new Java oracle run.
The test uses shared semantic `is_set`, with separate E09 tests covering general
value/provider default and assignment-state behavior.


## Inverse XMI oracle

`ObserveInverseXmi.java` uses the common/Ecore/XMI classpath pinned above. It emits
24 schema/XMI pairs and observations for six independent pairs per configuration.
Only uncontained children are added as resource roots, avoiding double ownership.

```powershell
java -cp $env:EMF_XMI_CLASSPATH tools/ecore-features/ObserveInverseXmi.java target/e09-inverse-xmi-oracle
python -B -m unittest discover -s tools -p 'test_ecore_inverse_xmi_oracle.py'
cargo test -p intersect-xmi --locked --test unsettable
```

The exact cache and hashes are in `fixtures/ecore-unsettable/emf-inverse-xmi`.
All EMF save/load cases preserve state; native tests match all 288 object states
and preserve complete models through native round trips. Independent EMF reload
verifies all 24 native exports, with a negative control proving that lost explicit
empty assignment state is detected. Java is verification-only.

```powershell
cargo run -p intersect-xmi --locked --example inverse_xmi_oracle -- target/e09-inverse-native-export
java -cp $env:EMF_XMI_CLASSPATH tools/ecore-features/ObserveInverseXmi.java target/e09-inverse-xmi-oracle target/e09-inverse-native-export
```

The identity-only ids.tsv files map native object IDs to oracle IDs. EMF observes
effective values, eIsSet and containment itself; the helper supplies no expected
semantic results. All loaded objects must occur exactly once in the mapping.


## Collection mutation oracle

ObserveCollections.java uses the pinned common/Ecore classpath and observes 16
independent changeable/unsettable/unique/ordered Many EString configurations.
Eleven transitions per case include missing removal, fresh empty clear, duplicate
addition, first-occurrence removal and reflective reset. Native tests compare all
176 observations; this does not qualify indexed edits or every value family.

```powershell
java -cp $env:EMF_CLASSPATH tools/ecore-features/ObserveCollections.java target/e09-collections-observations.json
python -B -m unittest discover -s tools -p 'test_ecore_collection_oracle.py'
cargo test -p intersect-core --locked --test collection_edits
```


ObserveIndexedCollections.java independently covers eight ordered EString Many
configurations and 16 transitions per case. This qualifies ordered indexed
operations only; Intersect rejects indexed edits on unordered features. Run with
the same pinned common/Ecore classpath:

```powershell
java -cp $env:EMF_CLASSPATH tools/ecore-features/ObserveIndexedCollections.java target/e09-indexed-collections-observations.json
```

Source/output/JAR hashes are in indexed-collections-provenance.json. The offline
collection oracle test verifies both matrices; native collection_edits compares
all 128 indexed snapshots in addition to the 176 value-based snapshots.


## Transient XMI projection oracles

ObserveTransientXmi varies both endpoints' transient/unsettable flags over all valid
scalar/Many and containment inverse shapes (96 configurations). Null after-object
observations mean the object was omitted, not a runtime-null feature value.
ObserveTransientValues records 17 additional value/default cases. Both use the
pinned common/Ecore/XMI classpath and unchanged cached bytes in fixtures/ecore-transient.

```powershell
java -cp $env:EMF_XMI_CLASSPATH tools/ecore-features/ObserveTransientXmi.java target/e09-transient-xmi-oracle
java -cp $env:EMF_XMI_CLASSPATH tools/ecore-features/ObserveTransientValues.java target/e09-transient-values-oracle
python -B -m unittest discover -s tools -p 'test_ecore_transient_oracle.py'
```

Native tests import the exact outputs and compare native projection from the full
pre-save runtime models with EMF's post-load observations. Independent EMF reload
now verifies all 113 native exports, with two mutation controls proving detection
of incorrect omission. Java stays verification-only.

```powershell
cargo run -p intersect-xmi --locked --example transient_xmi_oracle -- target/e09-transient-native-export
java -cp $env:EMF_XMI_CLASSPATH tools/ecore-features/ObserveTransientXmi.java target/e09-transient-xmi-oracle target/e09-transient-native-export/inverse
java -cp $env:EMF_XMI_CLASSPATH tools/ecore-features/ObserveTransientValues.java target/e09-transient-values-oracle target/e09-transient-native-export/values
```

ids.tsv maps native IDs to original IDs and contains no expected semantic values.
The EMF hosts reject missing/duplicate/unmapped objects and compare their own
post-load observations, including absence of transiently contained objects.


## Reference declaration identity and same-object links

`ObserveReferenceIdentity.java` uses the same pinned common 2.42.0/Ecore 2.38.0
verification-only classpath. `Diagnostician` rejects a reference declared as its
own opposite (code 11). Four valid shape/unsettable combinations instead pair
two distinct declarations on one class, then observe 16 fresh, cross-object,
same-object and displaced-self states. Runtime acceptance of an invalid schema
does not establish Ecore validity.

```powershell
java -cp $env:EMF_CLASSPATH tools/ecore-features/ObserveReferenceIdentity.java target/reference-identity.json
python -B -m unittest discover -s tools -p 'test_ecore_reference_identity_oracle.py'
```

The retained output and complete source/JDK/JAR digests are in
`fixtures/ecore-structure/reference-identity*.json`. Core tests replay all 16
states; definition tests reject direct/inherited-alias self-opposites in source
and checked schemas and preserve same-name distinct declarations. The oracle
does not qualify containment, delete, all cardinalities or generated delegates.


## Remove and delete observations

`ObserveDeletion.java` uses the same pinned common/Ecore/JDK classpath and compares
remove, nonrecursive delete and recursive delete with unsettable false/true over
two loaded resources. Six observations and exact source/output/tool identities
are retained in `fixtures/ecore-structure/deletion*.json`.

```powershell
java -cp $env:EMF_CLASSPATH tools/ecore-features/ObserveDeletion.java target/deletion.json
python -B -m unittest discover -s tools -p 'test_ecore_deletion_oracle.py'
```

EMF removes resource reachability but retained Java references keep the detached
objects alive. Intersect checked deletion removes selected records; its explicit
object-only policy rejects surviving contained children. Native qualification
compares the survivors of recursive deletion, not Java object lifetime or
automatic resource loading. Singular survivor references become unset; Many
removal preserves assignment history. This oracle does not qualify readonly,
custom delegates, proxies or all containment shapes.


## Nonunique opposite declaration matrix

`ObserveNonuniqueOpposites.java` checks 64 combinations of forward/reverse
singular/Many, uniqueness, containment and unsettable state using the same pinned
JDK and common/Ecore jars. EMF rejects 34 declarations: code 50 requires unique
containment/bidirectional references whenever upper bound differs from one; code
28 requires singular container back-references. Invalid declarations are never
executed. The 30 valid configurations each have six snapshots, including duplicate
addition, reverse assignment, single removal and unset. Provenance and output
are `fixtures/ecore-structure/nonunique-opposites*.json`.

```powershell
java -cp $env:EMF_CLASSPATH tools/ecore-features/ObserveNonuniqueOpposites.java target/nonunique-opposites.json
python -B -m unittest tools.test_ecore_nonunique_oracle
```

Native compiler/builder validity and all 180 valid snapshots are checked separately
from the cached observation integrity test. Nonunique singular references remain
valid; unidirectional noncontainment Many lists can still contain duplicates.

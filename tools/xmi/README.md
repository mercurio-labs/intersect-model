# Optional EMF interoperability oracle

This handwritten Java host is an independent verification tool only. No Java is
used by definition compilation, Rust builds, model import/export or production.
It loads the repository's Library Ecore dynamically with EMF, loads native XMI,
resolves references, checks six elements/one loan, and saves plain EMF XMI after
explicitly clearing extension metadata. The Rust harness compares every stored
feature/reference using class/name correspondence, excluding source spans and
resource-local ID spelling. Run from the repository root with JDK 17+ and these
three JARs on the classpath (use `;` on Windows, `:` on Unix):

```text
org.eclipse.emf:org.eclipse.emf.common:2.30.0
org.eclipse.emf:org.eclipse.emf.ecore:2.36.0
org.eclipse.emf:org.eclipse.emf.ecore.xmi:2.37.0
```

```sh
cargo run -p intersect-xmi --example emf_oracle --locked -- export target/xmi-oracle/input
java -cp "$EMF_CLASSPATH" tools/xmi/Verify.java . target/xmi-oracle/input target/xmi-oracle/emf
cargo run -p intersect-xmi --example emf_oracle --locked -- verify target/xmi-oracle/emf
```

The last verified run was 2026-10-06 on Windows, Oracle JDK 17.0.12+8-LTS-286.
JAR SHA-256 values, in the order above:

```text
4e4d516e2958a28b3dea3ab95bca91a14af3c3cebd11705d6027417a93178fde
bebb33f7683098ab65997e23fc1273c58bd0d39cc9334aaa6110abe240daa3f3
86607313d60acf901bee23378c9625a591b1f346a0b9c01c657a47e235d7e805
```

The input model comes from `fixtures/library/{library.ecore,library.xtext,
library-behavior.rcore,catalog.library,loans.library}` through the production native
evaluator. `fixtures/xmi/emf/resource-0000.xmi` and `resource-0001.xmi` are the
EMF-saved output from this run, retained as Java-free import controls. They are
our Library data serialized by EMF, not copied EMF implementation code.
The check does not establish general XMI compatibility or performance.

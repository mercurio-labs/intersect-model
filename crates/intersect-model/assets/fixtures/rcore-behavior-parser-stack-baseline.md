# Parser stack reference observations

Eight exact behavior-only/canonical Rcore source documents retain the complete
pre-repair AST and diagnostics at depths 32, 60, 63 and 64. Their Unicode source
bytes and identities are frozen. The required syntax limit remains 64; depth 64
retains its original rejection and recovery behavior.

The baseline parser is identical at clean develop 16b6f6be and the parameter
checkpoint 982d5ba6. Native capture used an eight-MiB thread only to obtain complete
reference outputs after the original one-MiB entry had independently overflowed.
The required repaired entry remains one MiB, including unoptimized native and
explicit-stack WASM controls. A larger capture stack is not acceptance.

A temporary, explicitly invoked observer captured every retained poll before any
production parser change. Each row is [phase, consumed tokens, created arena nodes,
ready flag]. Complete documents match the clean-baseline capture. The observer
source and logs remain in the primary checkout's ignored support evidence folder;
no original frozen compiler observation was regenerated. The temporary ignored
capture test is removed from the final candidate.

The new portable verifier pins the complete fixture bytes. Native controls compare
complete documents and exact poll histories across quanta 1, 3 and 256, and drop
every pending prefix before fresh recovery. The original one-MiB function compiler
matrix and all 51 earlier module/type/image observations remain required.
This evidence establishes a repair reference, not full S09 or language closure.

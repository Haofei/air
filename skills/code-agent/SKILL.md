# Code Agent

Use this skill for general software engineering tasks that require inspecting a
repository, editing files, running verification, and returning an auditable diff.

The skill runs through AIR, not as a free-form script. AIR resolves the workflow
profile, tool config, capability policy, trace, artifact, and replay metadata
from `air-skill.yaml`.

Default command:

```bash
air run "<task>"
```

Before changing files, locate the relevant files and symbols with search, glob,
LSP, or bounded span inspection. Avoid broad file loading. After changes, run a
task-relevant verification command and inspect the final diff.

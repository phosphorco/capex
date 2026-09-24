# CapEx

A fork of Codex that can progressively reveal skills and resources as hooks identify different capabilities that should be included.

Your project structure:

```txt
.codex/ (or .agents/)
	capabilities/
		<name>/
			CAPABILITY.md
				- frontmatter:
					- tags: list tags for resources to match
	skills/ (like normal)
		<name>/
			SKILL.md
				- new frontmatter:
					- metadata.tags: list tags that characterize who this skill is for
```

New env variables
```txt
CAPEX_CAPABILITY_MODE = 1 - Enable tag matching
CAPEX_INITIAL_CAPABILITIES = Specify the capabilities that will be automatically injected (like having multiple AGENTS.md), these selected capabilities automatically make their tags part of the "include tags" for selecting available skills.
CAPEX_INITIAL_INCLUDE_TAGS = Specify the tags of the skills you want show as available at the start of the session.
CAPEX_INITIAL_EXCLUDE_TAGS = Specify the tags of the skills you want to exclude (applied after the inclusions).
CAPEX_ALWAYS_EXCLUDE_TAGS = Specify the tags of the skills you want to _always_ exclude (applied after the inclusions).
```

Something about how hooks can enable capabilities by returning information to the session. This is meant to take heavy inspiration from workbench https://github.com/phosphorco/workbench-go - see https://github.com/phosphorco/workbench-go/blob/main/docs/agent-protocol.md

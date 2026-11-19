# Contributing to Rhizo

Thanks for your interest. Rhizo is an early proof of concept, so the most
valuable contributions right now are reviews of specs and design proposals.

## How work flows

Rhizo uses spec-driven development with
[OpenSpec](https://github.com/Fission-AI/OpenSpec):

1. **Propose.** Every new behavior starts as a change under
   `openspec/changes/<change-id>/` with a proposal, a design, delta specs and a
   task list.
2. **Review.** The change is discussed and approved in a pull request before
   any code is written.
3. **Implement.** Code is written following `tasks.md`, with tests.
4. **Archive.** Once merged, the change is archived and its specs become part
   of `openspec/specs/`, the description of how the system behaves today.

## Using AI coding agents

Contributions made with any AI coding agent are welcome: Claude, ChatGPT/Codex,
Gemini, DeepSeek, Grok, Qwen, local models or none at all. The repository stays
neutral:

- [`AGENTS.md`](AGENTS.md) is the single, shared set of instructions. It follows
  an open standard read by most coding agents.
- Tool-specific files are **not** committed. Each contributor generates them
  locally.

### Local setup for your tool

Install the OpenSpec CLI and generate the slash commands for your own agent:

```bash
npm install -g @fission-ai/openspec
```

```bash
openspec init --tools <your-tool>
```

Run `openspec init --help` for the list of supported tool ids (for example
`claude`, `codex`, `gemini`, `cursor`, `github-copilot`, `qwen`, `opencode`).
The generated files are ignored by git.

If your agent does not read `AGENTS.md` natively, point its own instruction
file at it locally. For example, Claude Code reads `CLAUDE.md`; create one
containing a single line:

```
@AGENTS.md
```

### Your responsibility

You are responsible for everything you submit, whoever or whatever wrote it:

- You must understand the code and be able to explain it in review.
- It must not contain code copied from copyleft (GPL, AGPL, LGPL) sources.
- Tests must pass locally before you open a pull request.

## Licensing of contributions

Rhizo is licensed under the [MIT License](LICENSE). By submitting a
contribution you agree that it is licensed under the same terms.

## Language

Code, comments, specs, commits and pull requests are written in English so the
project stays accessible to everyone. Discussions in issues may happen in other
languages if participants prefer.

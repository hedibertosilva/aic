# aic

CLI que orquestra o Claude Code e o Gemini (via [Antigravity CLI](https://antigravity.google/docs/cli/using/), `agy`) nas duas fases de um fluxo de feature: planejamento (discutir e revisar uma proposta em `.md`) e desenvolvimento (executar o plano aprovado e revisar o código resultante). Um agente codifica, o outro revisa — sem login em nenhum app de terceiros, tudo via CLI local.

Segue o modelo "config-first" do `/config` do Claude Code: você ajusta os defaults (quem revisa, quem executa, modo manual/auto) uma vez, e os comandos do dia a dia ficam curtos.

## Instalação

Requer Rust (`cargo`/`rustc`) e os binários `claude` e `agy` (Antigravity CLI) já instalados e autenticados.

```
cargo build --release
cp target/release/aic ~/.local/bin/aic   # ou outro diretório no seu PATH
```

> Se você compila dentro de um distrobox/container, copie o binário direto para o host em vez de usar `distrobox-export --bin` — o wrapper gerado roda o `aic` *dentro* do container a cada chamada, o que pode fazer suas próprias ferramentas (`git`, etc.) resolverem para dentro do container também.

## Configuração

`aic config` abre um menu interativo (como o `/config` do Claude Code) para ajustar os defaults, guardados em `~/.config/aic/config.toml`:

```toml
executor = "claude"              # quem executa planos / corrige issues
reviewer = "gemini"              # quem revisa planos / diffs
mode = "manual"                  # "manual" (uma rodada e para) | "auto" (loop até aprovar)
max_rounds = 3                   # teto do modo auto
permission_mode = "acceptEdits"  # "acceptEdits" | "plan"
default_base_branch = ""         # vazio = auto-detectar via origin/HEAD
skip_permissions = false         # opt-in: auto-aprova toda ferramenta em runs headless

[agents.claude]
binary = "claude"
model = ""
effort = ""

[agents.gemini]
binary = "agy"
model = ""
effort = ""
```

Um repositório pode sobrescrever qualquer chave em `.aic/config.toml` (mesmo schema, campos parciais).

Fora do menu interativo:

```
aic config show                  # config efetiva (global + override do repo)
aic config set mode auto
aic config set max_rounds 5
```

Todo comando abaixo aceita flags (`--reviewer`, `--executor`, `--mode`, `--max-rounds`, `--permission-mode`, `--target`, `--no-filter`) para sobrescrever a config numa chamada pontual — mas no uso normal eles não são necessários.

**`git_diff_exclude_patterns` / `git_diff_include_patterns`**: `aic dev review` filtra o diff antes de mandar pro reviewer, removendo lockfiles comuns (`Cargo.lock`, `package-lock.json`, `yarn.lock`, ...), arquivos ignorados pelo `.gitignore` e marcados como `linguist-generated` no `.gitattributes`. `git_diff_exclude_patterns` (lista, default `[]`) soma exclusões; `git_diff_include_patterns` re-inclui o que bater, com precedência sobre qualquer exclusão. Os padrões usam um subconjunto da sintaxe do gitignore: `*` e `?` (dentro de um segmento do caminho) e `**` (cruza `/`). Sem `/`, o padrão casa com qualquer segmento do caminho (`node_modules`, `*.min.js`); com `/`, é ancorado na raiz do repo e também cobre tudo dentro do diretório casado. Uma `/` final é ignorada. Não há negação com `!` (use `git_diff_include_patterns`) nem classes `[...]`. Use `aic dev review --no-filter` para ver o diff completo numa chamada.

**`skip_permissions`**: fica desligado por padrão. Sem ele, se o reviewer ou executor tentar usar uma ferramenta (ler um arquivo, rodar um comando) em modo headless, a chamada é negada e falha com um erro claro em vez de devolver uma resposta vazia silenciosa. Só ative se você confia nos agentes configurados para rodar sem supervisão nesse repositório.

## Comandos

| Comando | O que faz |
|---|---|
| `aic init` | Cria `.aic/` no repo atual (`plans/`, `dev/`, `adhoc/reviews/`, `state.json`) e o config global, se não existirem. Adiciona `.aic/` ao `.gitignore`. |
| `aic plan new <slug> [--title "..."]` | Cria `.aic/plans/<slug>/PLAN.md` a partir de um template (Problema, Proposta, Alternativas, Impacto, Questões abertas) e registra a feature. |
| `aic plan review <slug>` | Manda o `PLAN.md` pro reviewer; salva o relatório em `.aic/plans/<slug>/reviews/PLAN-RN.md`. Modo `auto` corrige e re-revisa até aprovar. |
| `aic dev execute <slug> [--mode interactive\|headless]` | Roda o executor contra o plano aprovado. `interactive` (default) assume o terminal (`exec`), igual uma sessão normal; `headless` roda via `-p` e loga em `.aic/dev/<slug>/EXECUTION-LOG.md`. |
| `aic dev review [<slug>]` | Revisa um diff: staged, commits desde o `dev execute`, ou contra a branch base — escolhido automaticamente, ou via `--target`. Com `<slug>`, salva em `.aic/dev/<slug>/reviews/CODE-RN.md`. Sem `<slug>`, é um review ad-hoc salvo em `.aic/adhoc/reviews/`. |
| `aic status [<slug>]` | Mostra o estado (plan/dev, rounds) de uma feature ou de todas. Sem argumento, tenta inferir a feature pelo nome da branch atual. |
| `aic config` / `config show` / `config set` | Ver/editar os defaults. |

`.aic/` fica fora do git (está no `.gitignore`): plano, relatórios e logs ficam só na sua máquina — copie manualmente pro PR o que for útil.

## Fluxo do dia a dia

Exemplo de ponta a ponta: um card de feature até o código revisado e pronto pra PR.

**1. Primeira vez no repo**

```
$ cd ~/code/payments-service && aic init
```

**2. Planejamento** — cria o esqueleto, discute e escreve a proposta normalmente (editor, ou pedindo ajuda ao Claude fora do `aic` — a conversa de design não é automatizada), depois pede a revisão:

```
$ aic plan new health-check --title "Health check endpoint"
$ aic plan review health-check
Round 1: approved=false — Falta checagem de timeout e as seções de
alternativas/impacto estão vazias.
```

Ajusta o `plan.md` e roda de novo até aprovar (ou usa `--mode auto --max-rounds 4` para deixar o loop corrigir sozinho).

**3. Executar o plano aprovado** — modo interativo acompanha o Claude trabalhando, igual uma sessão normal:

```
$ aic dev execute health-check
```

**4. Revisar o que foi implementado**:

```
$ git add -A
$ aic dev review health-check
Round 1: approved=false — Falta separar liveness de readiness.
```

Corrige (manual ou `--mode auto`) e roda de novo até aprovar. O `aic` sempre reavalia o diff real após cada correção — mesmo que não tenha sido comitada — então o loop converge em vez de repetir o mesmo diff.

**5. Conferir o estado** a qualquer momento:

```
$ aic status health-check
## health-check
  plan: approved (1 round(s))
  dev:  done (2 round(s))
```

Ou, dentro da branch da feature, sem passar o slug: `aic status` tenta inferir pelo nome da branch.

**6. Review ad-hoc**, sem vínculo com um plano (ex: um fix rápido):

```
$ git add -A && aic dev review
```

**7. Fechar o ciclo**: com `dev` em `done`, segue o fluxo normal — commit, push, PR.

## Arquitetura

```
src/
  main.rs            // parse clap, dispatch pros comandos
  cli.rs             // definição dos subcomandos/flags (clap derive)
  config.rs          // Config (toml, global + override de repo), merge, defaults
  state.rs           // State/Feature/Round (serde), .aic/state.json
  agent.rs           // CliAgent — invoca claude/agy como subprocesso, normaliza o
                     // envelope JSON de resposta (os dois têm formatos diferentes)
  review.rs          // run_review_loop() genérico — usado tanto por plan review
                     // quanto por dev review
  git.rs             // diffs, branch default, slug da branch atual
  templates.rs       // template do plan.md
  commands/          // init, plan, dev, status, config
```

`claude` e `agy` expõem flags headless quase idênticas (`-p`, `--output-format json`, `--json-schema`), mas o envelope de resposta e algumas flags (permissão de edição, prompt inicial em modo interativo) divergem — `agent.rs` e `commands/dev.rs` normalizam essas diferenças por binário, não por nome lógico do agente na config.

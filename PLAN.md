# `aic` — CLI harness para orquestrar Claude Code + Gemini (Antigravity CLI)

## Contexto

Hoje o fluxo do usuário é manual: (1) na fase de planejamento, discute uma feature,
escreve uma proposta em `.md`, e pede pro Gemini revisar — copiando/colando entre
ferramentas ou arquivos; (2) na fase de desenvolvimento, pede pro Claude ou Gemini
executar o plano e depois pede code review dos commits/staged changes, de novo com
interação manual entre as duas CLIs.

O objetivo é construir uma CLI (`aic`, de "ai-integrate") em **Rust** — projeto de
aprendizado explícito do usuário, que vai revisar o código gerado para aprender a
linguagem — que formaliza esse fluxo: cria e rastreia o plano, aciona o Gemini
(via `agy`, o Antigravity CLI) ou o Claude (via `claude`) para revisar, e aciona
qualquer um dos dois para executar/revisar código, guardando o estado localmente
fora do git (`.aic/` fica no `.gitignore` — artefatos ficam só na máquina, o
usuário copia manualmente o que quiser versionar).

Depois de uma primeira rodada de design, dois ajustes importantes:

1. **Config-first, não flag-first.** Em vez de cada comando levar meia dúzia de
   flags (`--reviewer`, `--executor`, `--mode`, `--max-rounds`, `--permission-mode`,
   `--target`...), a CLI segue o modelo do `/config` do Claude Code: existe um
   comando `aic config` interativo que define os defaults (quem revisa, quem
   executa, modo manual/auto, limite de rodadas, branch base), e os comandos do
   dia a dia ficam curtos (`aic plan review <slug>`, `aic dev execute <slug>`).
   Flags continuam existindo, mas só como *override pontual*, não como uso normal.
2. **Correções da revisão do plano original** (ver seção "Decisões revisadas"
   abaixo): distribuição do binário fora do distrobox, semântica do modo
   interativo de execução, ordem de implementação (validar o envelope JSON real
   do `agy` antes de fixar o parser), branch base auto-detectada, e tratamento de
   erro no loop de revisão.

Descobertas relevantes do levantamento:
- `claude -p "<prompt>" --output-format json --json-schema '<schema>'` e
  `agy -p "<prompt>" --output-format json --json-schema '<schema>'` têm, pela
  documentação, **a mesma interface headless** (print mode, output json/stream-json,
  json-schema para structured output, `--permission-mode`, `--effort`, `--model`).
  Isso permite uma única abstração `CliAgent` parametrizada por binário — mas essa
  suposição só foi confirmada rodando `claude --help` de verdade; `agy` ainda não
  foi testado (ver "Decisões revisadas").
- `claude` sem `--print` inicia sessão interativa já com o prompt inicial — ótimo
  para o modo "acompanhar no terminal" da execução do plano.
- Ambiente é Fedora Silverblue (imutável); Rust não está instalado no host nem no
  distrobox `apps-env` (container Fedora 41 já usado para dev geral). Vamos instalar
  o toolchain Rust (via `rustup`) **dentro do `apps-env`** em vez de criar um novo
  container — compilação acontece ali, mas o binário final precisa ser exportado
  pro host (ver "Decisões revisadas").
- `agy` (Antigravity CLI) ainda não está instalado — precisa ser instalado e
  autenticado (uma vez, via OAuth/keyring) **cedo**, para validar o formato real
  da resposta antes de implementar o parser.

## Decisões revisadas (depois da primeira versão do plano)

- **Distribuição do binário**: depois de `cargo build --release` dentro do
  `apps-env`, rodar `distrobox-export --bin ~/aic/target/release/aic` pra expor o
  binário no `PATH` do host Silverblue. Sem isso a CLI só funcionaria de dentro do
  container, inútil pro uso real do dia a dia em qualquer repo do host.
- **`dev execute --mode interactive`**: implementado com
  `std::os::unix::process::CommandExt::exec()`, que *substitui* o processo `aic`
  pelo processo do agente (`claude`/`agy`) — herda o terminal de verdade (sinais,
  stdin/stdout/stderr) sem precisar repassar manualmente. Consequência explícita:
  o `aic` não roda mais nenhum código depois do `exec()` — não há "pós-processamento"
  nesse comando. É por isso que o SHA do `HEAD` é gravado *antes* do exec, e por
  que `dev review` é sempre um comando separado, chamado depois.
- **Ordem de implementação**: instalar e autenticar o `agy` logo depois do
  `cargo new` (era o passo 9, virou passo 3), e capturar manualmente uma resposta
  real de `claude -p ... --output-format json --json-schema ...` e de
  `agy -p ... --output-format json --json-schema ...` pro mesmo prompt de teste,
  comparando as duas antes de escrever `agent.rs`. Se os envelopes divergirem
  (nomes de campo diferentes, por exemplo), o `CliAgent` ganha um pequeno mapeamento
  por agente em vez de assumir forma idêntica.
- **Branch base**: em vez de um valor fixo (`main`) na config, `git.rs` tenta
  `git symbolic-ref refs/remotes/origin/HEAD` pra descobrir a branch default real
  do remoto; só cai pro valor da config (`default_base_branch`, opcional) se isso
  falhar (ex: repo sem remoto).
- **Erros do agente no loop de revisão**: `review::run_review_loop` trata
  `status != SUCCESS` (erro, timeout, cancelado) como falha dura — aborta com
  mensagem clara, não entra no loop auto tentando "corrigir" um erro de execução
  como se fosse um issue de conteúdo.
- **Target default de `dev review`**: se houver staged changes, usa elas; senão,
  se a feature tem um SHA de início gravado por `dev execute`, usa o diff desde
  esse SHA até `HEAD`; senão, cai pro diff contra a branch base auto-detectada.

## Config (`aic config`) — o coração do "menos parâmetros"

`aic config` sem argumentos abre um menu interativo (via `dialoguer`), parecido
com o `/config` do Claude Code: lista as chaves atuais e deixa escolher uma pra
editar. `aic config show` imprime a config efetiva (global +, se existir, override
de repo) em formato legível. `aic config set <key> <value>` continua existindo
pra uso não-interativo/scriptável, mas é o caminho secundário.

Chaves de config (`~/.config/aic/config.toml`, com override opcional em
`.aic/config.toml` por repositório):

```toml
executor = "claude"       # agente usado por padrão pra executar planos / corrigir issues
reviewer = "gemini"       # agente usado por padrão pra revisar planos / diffs
mode = "manual"           # "manual" (uma rodada e para) | "auto" (loop até aprovar)
max_rounds = 3            # teto do modo auto
permission_mode = "acceptEdits"  # repassado como --permission-mode pro executor
default_base_branch = ""  # opcional; vazio = auto-detectar via origin/HEAD

[agents.claude]
binary = "claude"
model = ""                # vazio = usa o default do próprio claude
effort = ""

[agents.gemini]
binary = "agy"
model = ""
effort = ""
```

Com isso, o uso do dia a dia fica:

```
aic plan review auth-flow          # usa reviewer/executor/mode/max_rounds da config
aic dev execute auth-flow          # usa executor/mode/permission_mode da config
aic dev review auth-flow           # usa reviewer/executor/mode/max_rounds da config, target auto-detectado
```

E as flags por comando (`--reviewer`, `--executor`, `--mode`, `--max-rounds`,
`--permission-mode`, `--target`, `--range`) continuam existindo em todo comando
relevante, mas só pra *sobrescrever* a config numa chamada pontual — não são
necessárias no uso normal.

## Escopo da v1 (MVP)

Binário único `aic`, comandos:

1. **`aic init`** — cria `~/.config/aic/config.toml` (se não existir, com os
   defaults acima) e o esqueleto `.aic/` no repositório atual (`.aic/plans/`,
   `.aic/dev/`, `.aic/state.json`), adicionando `.aic/` ao `.gitignore` do repo
   (cria o arquivo se não existir, só faz append se já existir e a linha não
   estiver presente).

2. **`aic plan new <slug> [--title "..."]`** — cria `.aic/plans/<slug>/plan.md` a
   partir de um template fixo (Problema, Proposta, Alternativas consideradas,
   Impacto, Questões abertas) e registra a feature em `state.json` com status
   `draft`. Não automatiza a "discussão" — isso continua sendo uma conversa normal
   com o Claude/editor; o comando só formaliza onde o arquivo mora.

3. **`aic plan review <slug> [--reviewer <agente>] [--executor <agente>] [--mode
   manual|auto] [--max-rounds N]`** (todos os flags opcionais, default vem da
   config)
   - Lê `plan.md`, monta um prompt de revisão fixo + o conteúdo do arquivo, chama
     o `reviewer` em modo headless com `--json-schema` pedindo
     `{approved: bool, summary: string, issues: [string]}`.
   - Salva a resposta em `.aic/plans/<slug>/reviews/plan-rN.md` (timestamp,
     agente, summary, issues) e atualiza `state.json` (round, approved, status
     `approved`/`changes_requested`).
   - Modo `manual` (default): roda uma rodada e para — imprime o resultado pro
     usuário decidir o próximo passo.
   - Modo `auto`: se não aprovado, chama o `executor` em headless pedindo pra
     revisar o `plan.md` endereçando as `issues` (com `--permission-mode`
     configurado, restrito ao arquivo do plano), roda o reviewer de novo, repete
     até `approved=true`, `max_rounds` atingido, ou erro duro do agente (aborta).

4. **`aic dev execute <slug> [--mode interactive|headless] [--executor <agente>]
   [--permission-mode <mode>]`**
   - `interactive` (default): `exec()` no processo do `executor`, prompt inicial
     "execute o plano em `.aic/plans/<slug>/plan.md`" — usuário acompanha/participa
     normalmente; `aic` não roda mais nada depois disso nessa invocação.
   - `headless`: roda em `--print`, captura JSON, grava um registro de execução em
     `.aic/dev/<slug>/execution-log.md` (timestamp, status, resumo da resposta).
   - Em ambos os casos, grava no `state.json` o SHA do `HEAD` **antes** de começar
     — usado depois por `dev review` pra saber o range de commits da feature.

5. **`aic dev review [<slug>] [--target staged|commits|branch] [--range <rev>]
   [--reviewer <agente>] [--executor <agente>] [--mode manual|auto] [--max-rounds
   N]`**
   - Target default (sem `--target` explícito): staged se houver; senão commits
     desde o SHA gravado em `dev execute`; senão diff contra a branch base
     auto-detectada.
   - Sem `<slug>`: review ad-hoc, sem ligar a uma feature — salva em
     `.aic/adhoc-reviews/<timestamp>.md`.
   - Mesmo padrão de `--json-schema` para `{approved, summary, issues: [{file,
     description, severity}]}`, mesmo loop manual/auto do `plan review`.

6. **`aic status [<slug>]`** — lê `state.json` e imprime o estado (plan:
   draft/in_review/approved + rounds; dev: not_started/executing/review/done +
   rounds) de uma feature ou de todas. Sem `<slug>`, tenta inferir a feature atual
   pelo nome da branch git (slugificado) antes de listar todas.

7. **`aic config`** (interativo, sem args), **`aic config show`**, **`aic config
   set <key> <value>`** — conforme seção "Config" acima.

Em todo comando que recebe `<slug>` como opcional/primeiro argumento posicional
(`dev execute`, `dev review`, `status`), se omitido a CLI tenta inferir pelo nome
da branch git atual (slugificado) e só pede explicitamente se não achar uma
feature correspondente em `state.json`.

Fora do escopo da v1 (deixar pra depois, não implementar agora): múltiplos
repositórios/workspaces simultâneos, integração com GitHub PRs, suporte a mais
agentes além de `claude`/`agy`, templates de plano customizáveis por projeto.

## Arquitetura / módulos

Crate binário `aic` (edição 2021), dependências principais: `clap` (derive),
`serde`/`serde_json`, `toml`, `anyhow`, `chrono`, `dialoguer` (menu interativo do
`config` e confirmações no modo manual), `owo-colors` (saída colorida).

```
src/
  main.rs            // parse clap, dispatch pros handlers de comando
  cli.rs             // structs/enums do clap (Cli, Commands, subcomandos; todos os
                     // flags de override são Option<T>, default = None = "usa a config")
  config.rs          // Config (toml, global + override de repo), load/save/merge, defaults,
                     // menu interativo (dialoguer) pra `aic config` sem args
  state.rs           // State/Feature/Round (serde), load/save .aic/state.json
  agent.rs           // trait AgentRunner (pra permitir fake nos testes) + CliAgent (impl real
                     // via std::process::Command), RunOpts, parse do envelope JSON
  review.rs          // ReviewVerdict { approved, summary, issues }, schema fixo,
                     // run_review_loop() genérico (usado por plan review e dev review),
                     // trata status != SUCCESS como erro duro (não "not approved")
  git.rs             // diff_staged(), diff_range(a,b), diff_against_branch(name),
                     // default_branch() (via origin/HEAD, com fallback pra config),
                     // head_sha(), is_repo(), current_branch_slug()
  templates.rs       // template fixo do plan.md
  commands/
    init.rs
    plan.rs          // new, review
    dev.rs           // execute, review
    status.rs
    config_cmd.rs
```

`CliAgent::run` é o ponto único que invoca `claude`/`agy` como subprocesso —
monta os args conforme `RunOpts`, captura stdout/stderr, devolve
`AgentRunResult { status, raw_text, json: Option<Value> }`. Tanto `plan review`
quanto `dev review` chamam `review::run_review_loop`, passando o "conteúdo a
revisar" (texto do plano ou diff), o prompt-base específico, e o par
executor/reviewer resolvidos (flag > config de repo > config global) — a lógica
de loop é compartilhada, não duplicada entre os dois comandos.

Erros: `anyhow::Result` em todo lugar, com contexto (`.context("...")`) nos
pontos de I/O e de chamada de subprocesso — inclui mensagem clara se o binário
configurado não for encontrado no PATH.

## Passos de implementação

1. Instalar Rust dentro do `apps-env` (distrobox): `distrobox enter apps-env --
   bash -lc 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s --
   -y'`.
2. `cargo new aic` em `/var/home/hediberto/Workspace/ai-integrate` (`git init`
   também, pra versionar o código da própria CLI).
3. Instalar e autenticar o `agy` (Antigravity CLI) — passo manual do usuário
   (abre browser uma vez). Rodar manualmente `claude -p "..." --output-format
   json --json-schema '...'` e `agy -p "..." --output-format json --json-schema
   '...'` com o mesmo prompt/schema de teste, comparar os dois envelopes JSON
   reais antes de seguir — isso define a forma exata de `AgentRunResult`.
4. Adicionar dependências no `Cargo.toml`.
5. Implementar `config.rs` + `state.rs` (modelos + load/save/merge) e testes
   unitários de (de)serialização, incluindo o merge global+repo.
6. Implementar `agent.rs` (trait `AgentRunner` + `CliAgent`, monta args, roda
   subprocesso, parse do envelope) com testes unitários do parsing usando as
   respostas reais capturadas no passo 3 como fixtures.
7. Implementar `git.rs` (incluindo `default_branch()` via `origin/HEAD`) com
   testes contra um repo git temporário criado em `tempdir`.
8. Implementar `review.rs` (loop genérico, incluindo o tratamento de erro duro)
   com testes usando um `AgentRunner` fake.
9. Implementar os comandos (`init`, `plan new/review`, `dev execute/review`,
   `status`, menu interativo de `config`) ligando os módulos acima.
10. `distrobox-export --bin` do binário release pro host.
11. Smoke test manual: `aic init` → `aic plan new demo` → editar `plan.md` →
    `aic plan review demo` (chama `agy` de verdade, usando defaults da config) →
    `git add` algo de teste → `aic dev review demo` → `aic status`.

## Verificação

- `cargo build` e `cargo test` (dentro do `apps-env`) depois de cada módulo
  implementado — cobre parsing de config/state/agent-envelope/git diff sem
  depender de rede, usando fixtures reais capturadas no passo 3.
- Teste manual end-to-end (passo 11 acima) com `agy` e `claude` reais, num repo
  de teste descartável, confirmando que: os arquivos em `.aic/` são criados
  corretamente; `.aic/` não aparece em `git status`; o `aic` exportado via
  `distrobox-export` roda direto do shell do host, fora do container.

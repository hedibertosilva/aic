# aic

O `aic` é um ambiente local para trabalhar com Claude Code, Gemini (via
Antigravity CLI, `agy`) e Codex no mesmo repositório. Ele oferece um chat web
local para comparar respostas e um fluxo de desenvolvimento que separa
planejamento, implementação e revisão.

Tudo passa pelas CLIs dos provedores. O aic não cria uma conta intermediária,
não envia o código para um serviço próprio e mantém conversas, planos e estado
no checkout local.

## Por que usar

- Converse com um ou vários agentes na mesma tela e compare perspectivas.
- Dê um papel claro a cada participante: implementador, revisor, pesquisador
  ou especialista de domínio.
- Comece em modo somente leitura e habilite edição apenas para quem precisa.
- Registre planos, revisões, tarefas e histórico dentro do próprio checkout.
- Execute agentes em Docker ou Podman para reduzir o alcance de alterações
  acidentais fora do repositório.
- Reaproveite sessões, exporte conversas e retome o trabalho sem perder o
  contexto visível.

## Requisitos

- Linux x86_64 ou arm64 para os binários publicados.
- Git e acesso à rede.
- Pelo menos uma CLI de agente instalada e autenticada: `claude`, `agy` ou
  `codex`.
- Docker ou Podman para o sandbox padrão. É possível escolher `none`, mas os
  agentes serão executados diretamente no host.

Rust só é necessário para compilar a partir do código-fonte. macOS, Windows e
outras arquiteturas não fazem parte do pipeline atual de releases.

## Instalação rápida

Baixe um arquivo da página de [Releases](https://github.com/hedibertosilva/aic/releases),
verifique o checksum e instale o binário:

```sh
version=VERSION
target=x86_64-unknown-linux-musl # arm64: aarch64-unknown-linux-musl
archive="aic-$version-$target.tar.gz"
curl -fLO "https://github.com/hedibertosilva/aic/releases/download/v$version/$archive"
curl -fLO "https://github.com/hedibertosilva/aic/releases/download/v$version/SHA256SUMS"
awk -v archive="$archive" '$2 == archive { print }' SHA256SUMS > "$archive.sha256"
sha256sum -c "$archive.sha256"
tar -xzf "$archive" aic
mkdir -p "$HOME/.local/bin"
install -m 755 aic "$HOME/.local/bin/aic"
export PATH="$HOME/.local/bin:$PATH"
```

Para compilar manualmente, consulte [compilação a partir do código-fonte](docs/build-from-source.md).

## Primeiro repositório

Execute o setup dentro do checkout que será usado:

```sh
cd /caminho/do/seu/repositorio
aic setup
aic
```

O setup verifica o ambiente, prepara o sandbox, encontra as CLIs, orienta o
login e faz um teste real mínimo antes de permitir a escolha do agente padrão.
Esse teste pode consumir cota. O processo pode ser interrompido e retomado com
`aic setup`.

Se preferir executar as etapas manualmente:

```sh
aic init
aic sandbox build
aic agent login claude       # ou: gemini, codex
aic agent test claude
aic
```

O `aic` sem subcomando inicia o chat e executa `aic init` automaticamente.
O servidor escuta em `127.0.0.1:7878` por padrão. Use `aic chat --no-open`
em uma máquina sem navegador e abra a URL exibida manualmente. Não use
`--host 0.0.0.0` sem entender que qualquer pessoa com acesso à porta poderá
operar os agentes e as credenciais montadas.

## Fluxo recomendado para um novo trabalho

1. Abra o repositório correto e confirme o branch no painel **Git context**.
2. Crie uma conversa e escolha um agente pronto em **Add participant**.
3. Comece com uma pergunta de leitura: peça diagnóstico, alternativas e riscos.
4. Adicione participantes para obter uma segunda opinião; dê a cada um um papel.
5. Mantenha análise em somente leitura. Habilite **Can edit** apenas para quem
   implementará e descreva o resultado esperado.
6. Para uma mudança maior, use **Link or create plan**, registre objetivo e
   critérios de aceite e discuta antes de executar tarefas.
7. Revise o diff, rode os testes e valide cada tarefa. A resposta do agente não
   substitui a validação humana.
8. Exporte a conversa ou deixe o plano e os registros em `.aic/` para retomar.

Para um fluxo orientado por arquivos:

```sh
aic workflow plan new minha-feature --title "Minha feature"
aic workflow plan review minha-feature
aic workflow dev execute minha-feature
aic workflow dev review minha-feature
aic workflow status minha-feature
```

`plan review` consulta o revisor configurado. `dev execute` implementa um
plano aprovado. `dev review` analisa o diff; no modo `auto`, o executor pode
corrigir problemas e repetir o ciclo até o limite configurado. Para uma revisão
avulsa, omita o slug: `aic workflow dev review`.

## Participantes e skills

Um agente é uma integração configurada (`claude`, `gemini` ou `codex`). Um
participante é uma instância dessa integração dentro de uma conversa, com nome,
modelo, esforço, papel e acesso a arquivos. Uma conversa suporta até cinco
participantes.

- Use nomes e papéis objetivos, como `Implementador` e `Revisor`.
- Selecione destinatários em **To:** ou use `@nome` no início da mensagem.
- Envie a mesma pergunta a vários participantes para comparar respostas e, ao
  final, use **Ask them to comment on each other's replies**.
- Use `/fresh` para reiniciar sessões selecionadas sem replayar o histórico;
  `/clear` limpa a conversa; `/fork` cria uma direção alternativa.
- Skills aparecem ao digitar `/`. Envie uma skill para exatamente um
  participante. Ele precisa aceitar skills e ter edição habilitada quando a
  skill escrever arquivos; `agy` não executa skills.
- Confira argumentos, papel e diff produzido por uma skill antes de aceitar
  alterações.

## Configuração essencial

`aic config` abre o menu interativo. A configuração global fica em
`~/.config/aic/config.toml`; um `.aic/config.toml` no repositório pode
sobrescrever qualquer parte dela.

```sh
aic config show
aic config set chat.default_agent claude
aic config set workflow.executor claude
aic config set workflow.reviewer gemini
aic config set workflow.mode manual       # ou auto
aic config set sandbox.runtime docker     # docker, podman ou none
```

O sandbox monta o checkout para o agente, mantém o revisor em modo somente
leitura e limita o acesso ao restante do host. A rede e as credenciais
necessárias continuam disponíveis; revise prompts externos e arquivos antes de
executar agentes com permissão de escrita.

## Comandos úteis

| Comando | Uso |
| --- | --- |
| `aic setup` | Preparar ambiente, agentes e agente padrão. |
| `aic doctor` | Diagnosticar sem gerar resposta do provedor. |
| `aic agent list` | Listar CLIs, modelos e papéis configurados. |
| `aic agent status` | Consultar cotas reportadas pelos provedores. |
| `aic init` | Criar a estrutura local do aic no checkout. |
| `aic chat` | Iniciar o chat web local. |
| `aic workflow ...` | Usar plano, execução e revisão. |
| `aic config ...` | Consultar ou alterar padrões. |
| `aic sandbox build` | Criar ou atualizar a imagem do sandbox. |

## Documentação

O [índice de documentação](docs/README.md) separa guias de uso de contratos e
documentos de implementação. Comece por [setup e recuperação](docs/onboarding.md),
[worktrees](docs/worktrees.md) e [planos no chat](docs/chat-plans.md).

Para desenvolver o próprio aic, veja [compilação a partir do código-fonte](docs/build-from-source.md).

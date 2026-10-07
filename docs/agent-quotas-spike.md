# Spike: quotas e limites dos agentes no chat

Data: 2026-10-03. Entrega de descoberta técnica para o item 5 do `TODO`.

## Escopo confirmado

Somente `aic chat` web. Aviso fixo em **≥80%**; esgotado em **≥100%**.
Sem cota disponível, mostrar consumo observado da conversa por agente e
“cota não disponível”. Tokens ou custo ausentes devem continuar ausentes,
com indicação “não informado”, nunca ser estimados como zero.
Comando CLI, billing, histórico, limites próprios e configuração do limiar
ficam fora desta v1.

## Ambiente e método

O ambiente desta investigação é o próprio sandbox Debian 13/trixie, com
Node disponível e estas versões instaladas:

| CLI | Versão |
| --- | --- |
| Claude Code | 2.1.287, igual à versão fixada em `src/templates.rs` |
| Codex | 0.160.0 |
| agy / agente configurado como `gemini` | 1.2.16 |

Foram usados help, documentação dos fornecedores, inspeção dos parsers do
repositório, schema gerado pelo binário Codex e consultas sem prompts de
geração. Não se tentou esgotar uma conta para produzir erros reais de cota.
Claude não está autenticado; agy também não. Codex está autenticado e sua
consulta de cota funcionou. Não há Docker/Podman aqui para testar novamente
o lançamento do container a partir do host.

## Matriz agente × capacidade

| Agente | Origem de cota | Validação no sandbox | Custo e latência | Erro de limite e decisão para v1 |
| --- | --- | --- | --- | --- |
| Codex | `codex app-server` por stdio, método `account/rateLimits/read`; percentuais, duração e reset em `primary`/`secondary` | **Consulta autenticada confirmada**: retornou janelas de 300 e 10080 minutos | Uma leitura levou **2304 ms**, incluindo inicialização. Não iniciou thread/turno de geração; cobrança específica da consulta não foi medida | `exec --json` tem `turn.failed` e `error`; preservar mensagem estruturada. **R1 habilitável** para contas que retornem janelas; R2 caso não retornem |
| Claude | `claude -p "/usage"`: o evento `assistant` do `stream-json` traz `usage_report.rate_limits.limits[]` (`kind`, `percent` usado 0-100, `resets_at` RFC 3339, `severity`). O `rate_limit_event` do turno continua útil para a fase 2 | **Validado no host em 2026-10-03** (2.1.287, logado): janelas `session` (5h) e `weekly_all`. Não validado dentro do sandbox | **1,5 s** (`json`) e 3 s (`stream-json`); 0 tokens e US$ 0, sem chamada ao modelo. Sem `--no-session-persistence`, o `/usage` grava uma sessão em `~/.claude/projects` | Contrato não documentado oficialmente (só observado na 2.1.287, a versão fixada). `scope` veio `null`; janelas por modelo não foram observadas. **R1 na fase 1** |
| gemini / agy | `agy --print "/usage" --output-format json` (ou `/quota` em `stream-json`): `command.data.groups[].buckets[]` com `window`, `remaining_fraction` e `reset_time` | **Validado no host por Hediberto em 2026-10-03** (1.2.16): JSON e `stream-json` (evento `command_result`). Não validado dentro do sandbox | `duration_seconds: 0` e 0 tokens; latência de parede não medida. Sem login, a consulta cai em OAuth e expira (60 s) | Cota **por grupo de modelos**, não por agente: no teste, `Gemini Models` estava em 100% e `Claude and GPT models` em 0%. O texto de `response` mostra o percentual *restante*, não o usado. Falha real de limite ainda não capturada. **R1 na fase 1** |

Os tempos da matriz são amostras únicas, não benchmarks. Uma segunda
consulta Codex, usando o probe salvo, levou **3495 ms** e também retornou
as duas janelas. O probe passou na checagem de sintaxe e retornou erro
`ENOENT` com exit 1 ao receber um caminho de binário inexistente.
Executar um comando no
trixie prova compatibilidade do binário, mas não resolve credenciais
ausentes ou keyring do host.

## Evidências e mecanismos

### Codex

O [App Server oficial](https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt)
documenta a leitura e a notificação `account/rateLimits/updated`. A sequência
é `initialize` → `initialized` → `account/rateLimits/read`, sem `turn/start`.
Usar a CLI evita implementar extração/renovação de tokens no aic.
O [probe reproduzível](agent-quotas-probe.cjs) implementa essa sequência,
timeout e saída restrita a janelas, fonte e horários:

```sh
node docs/agent-quotas-probe.cjs
```

A leitura local retornou percentual e reset nas duas janelas. Reset está
em segundos Unix. Dar preferência ao mapa `rateLimitsByLimitId`, com
fallback para `rateLimits`, para não perder buckets adicionais.
Autenticação/API/providers sem janelas entram no fallback R2; não presumir
que toda instalação tem cota de assinatura.

O schema gerado por `codex app-server generate-json-schema --out <dir>`
inclui `usageLimitExceeded` e `rateLimitExceeded` em `CodexErrorInfo`.
Esses códigos são do **app-server**; não presumir que chegam iguais no
`exec --json` usado pelo chat. O [modo não interativo oficial](https://learn.chatgpt.com/docs/non-interactive-mode#make-output-machine-readable)
documenta `turn.failed`, `error` e consumo em `turn.completed.usage`.
Ainda falta captura de uma falha real de cota no exec para validar seus
metadados específicos de reset.

### Claude

O [parser do SDK mantido pela Anthropic](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/message_parser.py)
reconhece `type=rate_limit_event`, com `rate_limit_info.status`,
`resetsAt`, `rateLimitType` e `utilization`. Os
[tipos oficiais do SDK](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/types.py)
definem `allowed`, `allowed_warning` e `rejected`, e utilização como fração.
Converter utilização para percentual, mantendo ausência como ausência.
Um aviso do fornecedor sem percentual não autoriza inventar “80%”.

A [status line oficial](https://code.claude.com/docs/en/statusline#rate-limit-usage)
documenta percentuais e reset de 5h/7d, opcionais após a primeira resposta
de API. Esse é um payload da TUI; sua execução em `--print` não está
confirmada. Portanto não serve, por si só, como consulta prévia do chat.
O binário instalado contém `rate_limit_event` e `/api/oauth/usage`;
isso confirma indícios de implementação, não um contrato público de
consulta. Não usar esse endpoint privado como dependência da v1.

No código atual, `parse_claude` ignora o evento e os erros em
`result.errors` quando falta `result.result`; ambos precisam de tratamento.
Um evento `rejected` deve atualizar a cota; o encerramento do turno deve
continuar seguindo o resultado final, sem encerrar prematuramente a leitura.

### agy

O fornecedor [documenta `/usage` e `/quota`](https://www.agy.dev/docs/cli/commands/usage/)
como painel que atualiza cota no backend. A
[status line](https://www.agy.dev/docs/cli/statusline#available-json-fields)
documenta `quota[bucket].remaining_fraction` e `reset_time`. Converter para
percentual usado como `100 × (1 − remaining_fraction)`. Não confundir
`context_window.used_percentage` com cota da conta.

Os dois comandos testados falharam antes da consulta por falta de login.
O resultado stream real foi `event=result`, com `status=ERROR` e
`error="authentication failed or timed out"`; isso é erro de autenticação,
**não** de cota. É preciso uma conta autenticada para saber se `/usage`
em `--print` fornece JSON utilizável ou apenas representação textual.

A [documentação headless](https://www.agy.dev/docs/cli/headless/)
mostra `result.error`, contadores de tokens e contadores cumulativos com
input streaming. O binário 1.2.16 inclui changelog sobre `AGY_ERROR: {...}`
em stderr e exit 3 para falha de API/modelo; o schema e uma captura real de
cota desse caminho continuam pendentes. `Individual quota reached.` no
teste Rust atual é uma fixture, não evidência de execução nesta investigação.

## Implicações para o código e o modelo

O `Usage` atual é reaproveitável, mas não cobre sozinho a iniciativa:

- `src/chat/store.rs::parse_codex` descarta `turn.completed.usage` e só lê
  `turn.failed.error` como string. Deve aceitar também `error.message`
  e preservar o erro terminal; um `error` intermediário pode ser retry.
- `Usage::from_result` só reconhece o nome Claude para cache. agy usa
  `cache_read_tokens`; Codex usa `cached_input_tokens`. A normalização deve
  verificar se cache já está incluído no input para não somá-lo duas vezes.
- Confirmar escopo dos contadores com retomada de sessão. Não somar
  resultados cumulativos como se fossem deltas independentes; se cumulativos,
  calcular delta por sessão nativa, com tratamento de reset.

  **Validado em 2026-10-03 com Codex 0.160.0:** dois `codex exec --json`
  na mesma sessão retornaram totais cumulativos. O primeiro evento informou
  14172 tokens de entrada, 12288 em cache e 5 de saída; após `exec resume`,
  o segundo informou 28368, 24576 e 10. A implementação deve, portanto,
  manter baseline por sessão e contabilizar apenas o delta do segundo turno.
- `src/chat/web/chat.js::launch` salva `from`, mas não o agente/modelo do turno.
  Registrar esses valores na entrada ao iniciar a execução; troca de agente,
  renomeação e remoção de participante não podem reatribuir gastos antigos.
  Entradas antigas sem essa informação ficam como consumo de origem desconhecida.
- A conversa pode ser copiada, apagada e ter respostas regeneradas. Definir
  consumo da sessão como consumo **executado nesta conversa**: cópia de
  histórico não gera gasto e exclusão de uma mensagem não devolve tokens.
  Guardar totais por agente independentemente do log exibido. Incluir falhas
  com uso informado; falta de uso produz totais parciais, com indicação explícita.

Proposta de contrato, ainda não implementada:

```text
AgentQuota {
  agent, state: available | unavailable | read_error,
  windows: [QuotaWindow],
  source?, fetched_at?, attempted_at?, error?
}
QuotaWindow {
  id, label?, model?, used_pct?, window_minutes?, resets_at?, provider_status?
}
QuotaFailure { agent, kind: quota | rate_limit, reason, resets_at? }
```

Os campos do fornecedor são opcionais; identidade, estado e coleção de
janelas têm significado explícito. Exigir `source` e `fetched_at` quando
o estado for `available`. `windows=[]` não significa zero usado.
`fetched_at` marca a leitura bem-sucedida; falha não deve atualizar a hora
de dados antigos. Guardar `attempted_at` separadamente e indicar leitura
desatualizada. Uma janela vencida precisa de nova leitura, não de reset
local automático para 0%.

Preservar todas as janelas com IDs estáveis, em vez de um único
`QuotaStatus { window, used_pct }`. A cota é da conta/bucket do fornecedor;
o consumo observado é da conversa no aic. Vários participantes com o
mesmo agente podem compartilhar a mesma cota.

## Encaminhamento para a v1 e para UX

R1 pode começar com Codex e eventos Claude quando completos; R2 garante
presença de todos os agentes, inclusive agy e Claude antes da primeira
leitura válida. Isto não reduz o requisito: explicita os pontos em que a
fonte ainda não fornece dado confiável.

Para Vera, o contrato precisa comportar duas janelas, dados parciais e
erro de leitura além dos quatro estados visuais propostos. Sugestão:
resumo compacto por agente no header, detalhes expansíveis de todas as
janelas com fonte/hora/reset, e o mesmo resumo junto ao seletor. Consumo
da conversa deve aparecer também quando há cota, pois mede outra coisa.
Escolher a janela mais crítica para o resumo e mostrar as demais no detalhe.

Mensagem de falha: “Limite do {agente} atingido. {motivo}. Reseta em
{horário}, se informado.” Oferecer “Trocar de agente” usando a seleção
existente; sem reset conhecido, omitir o horário. Distinguir limitação
temporária de requisições de cota esgotada, e ambas de autenticação ou rede.

Atualização manual e ao fim do turno devem consultar fora da execução de
geração, com timeout, cache e deduplicação por agente. Nunca enviar um
prompt ao modelo para perguntar pela própria cota. Falha de consulta não
pode bloquear o turno nem apresentar uma cota antiga como atual.

## Validação restante antes de habilitar adapters adicionais

1. Capturar `rate_limit_event` autenticado do Claude em `--print`, incluindo
   quais percentuais/janelas realmente chegam e se falta `utilization`.
2. Capturar `/usage` autenticado do agy em JSON e stream-json e verificar
   formato, duração, ausência de geração e eventual uso de keyring.
3. Capturar falhas reais de limite de cada CLI quando ocorrerem naturalmente;
   validar resets e classificação de stderr sem provocar consumo excessivo.
4. Conferir deltas com duas chamadas retomadas de cada CLI e um reset de
   sessão, além da semântica de cache/tokens.
5. No host, executar os probes pelo caminho `Sandbox::run_args`, com os
   mounts reais: Claude recebe estado e credenciais, Codex recebe `.codex`,
   agy recebe um `.gemini` próprio do aic (login separado, em arquivo; o do host fica no keyring). Credenciais somente em keyring podem exigir outro
   mecanismo; o bind atual não prova que esse caso funciona.

O spike entregou a matriz, probe Codex e decisões técnicas. A fase 1 foi
implementada posteriormente: adapter Codex, cache do servidor, contadores
SQLite e interface. O escopo consolidado está em
[agent-quotas-ux.md](agent-quotas-ux.md).

## Atualização: leitura de Claude e agy via `/usage` (2026-10-03)

Os dois CLIs expõem o `/usage` em modo não interativo, com JSON estruturado,
sem gerar tokens. Isso substitui a leitura passiva por `rate_limit_event` como
caminho principal e move os dois para a fase 1, no mesmo modelo do Codex:
leitura sob demanda e cache no servidor. Implementado em `src/chat/quotas.rs`
(`read_claude`, `read_agy`), com as capturas reais como fixtures.

- **Claude:** `claude -p /usage --no-session-persistence --output-format stream-json --verbose`.
  Lido de `usage_report.rate_limits.limits[]`. Verificado de ponta a ponta pelo
  `aic chat` com o Claude real (7% sessão, 8% semana, nenhuma sessão gravada).
- **agy:** `agy --print /usage --output-format json`, lido de
  `command.data.groups[].buckets[]`; `used_pct = 100 × (1 − remaining_fraction)`.
  Cada janela leva a lista de modelos do grupo (`models`), extraída de
  `Models within this group: …`. O chat escolhe as janelas do modelo do
  participante (todas as palavras do item do grupo presentes no id do modelo,
  p. ex. `Gemini Pro` ↔ `gemini-3-pro-preview`). **Modelo que nenhum grupo
  reconhece mantém todas as janelas**: nunca esconder um limite por falha de
  nome. O painel por agente mostra o grupo mais crítico, nomeado.
- O `reset_time` de um bucket sem uso se desloca entre leituras (18:37:32 →
  18:37:43): janela deslizante. Uma mudança de `resets_at` não prova reset.
- **Ainda não validado:** execução dentro do sandbox (credenciais montadas),
  agy autenticado pelo aic (a leitura do agy foi só contra fixture; o JSON
  real foi capturado pelo usuário no host), janelas por modelo do Claude
  (`scope`), valores de `severity` diferentes de `normal`, e a falha real de
  limite (R5/fase 2). O agy com o grupo Gemini esgotado é a oportunidade
  de capturar essa falha sem custo.

## Alinhamento técnico com a proposta de UX da Vera

A combinação painel por agente + alertas nos participantes + nota abaixo
do select corresponde à estrutura atual. Manter os textos em inglês e
usar `in this conversation` para consumo executado nesta conversa. A
contabilização exige os ajustes acima; somar diretamente o log exibido
não cobre exclusão, regeneração, cópia e troca de agente.

Normalizar `resets_at` como instante absoluto UTC, preferencialmente
RFC 3339 no contrato web. Codex fornece segundos Unix; Claude documenta
segundos Unix; agy documenta timestamp em `reset_time` e duração auxiliar.
Quando só houver duração, calcular o instante a partir da hora da leitura.
O navegador formata dia/hora no fuso local e recalcula a parte relativa.

Manter a linha principal por agente, com detalhes por bucket/modelo.
Uma cota compartilhada não deve ser duplicada como cota independente de
cada modelo. A consulta Codex testada trouxe bucket de conta `codex`;
agy documenta chaves por modelo/bucket. Claude identifica janela no evento.

`launch(c, [destino], texto, [destino.name])` aceita envio direcionado,
mas `promptFor` acrescenta mensagens ainda não vistas pelo destino.
`launch` sem texto pode não reenviar uma mensagem já vista, e respeita
destinatários originais. A entrada da falha não guarda uma referência
explícita à mensagem que a originou, e um turno pode responder a várias
mensagens. Portanto **recomenda-se omitir “Send to…” na v1**: não há
reenvio fiel pronto. “Change agent…” pode reutilizar `openDialog(p)` e
depois focar `pagent`; “Try again” pode reutilizar `regenerate` com as
restrições atuais (última resposta do participante e conversa sem execução).

Falha confirmada de cota atualiza imediatamente o bucket afetado como
esgotado, mesmo sem percentual. **Rate limit transitório não implica
cota esgotada**, e erro genérico/HTTP 429 isolado não informa qual janela
esgotou. A distinção existe nos schemas documentados, mas ainda requer
capturas reais no stream usado pelo aic. Se não há evidência específica,
preservar a mensagem e não inventar janela, percentual ou reset.

A hora da leitura deve aparecer por agente; um horário geral não pode
parecer atualizar agentes cujas leituras falharam. Na ausência de `Usage`
após execução, mostrar `Usage not reported`, em vez de `No usage yet`.
No diálogo, preferir `Replies may fail until…` à promessa de que todas
as respostas falharão: a limitação pode afetar somente um bucket/modelo.

# Usage limits — especificação de UX

Escopo aprovado por Evandro e pelo usuário: chat web, interface em inglês,
aviso fixo em ≥80%, sem bloquear seleção, salvamento ou envio. “Send to…”
fica fora da v1. Descoberta técnica: [agent-quotas-spike.md](agent-quotas-spike.md).

## Entregas

1. **Fase 1:** consumo por conversa/agente no SQLite, agente gravado por
   turno, parser Codex corrigido, cota de Codex, Claude (`/usage`) e agy
   (`/usage`, por grupo de modelos), cache em memória no servidor, painel,
   badges e nota no seletor. Claude e agy entraram na fase 1 depois que o
   `/usage` de ambos foi validado (ver o spike).
2. **Fase 2:** cartão de falha classificado (cota × rate limit), após a ordem
   dos eventos e uma falha real de limite serem capturadas.
3. **Fase 3:** *(absorvida na fase 1 para o agy.)*

Um agente cuja leitura falhe, ou cuja conta não tenha janelas, usa os estados
explícitos e o consumo observado. No agy, o painel e os alertas usam as janelas
do grupo que governa o modelo do participante; modelo não reconhecido mantém
todas as janelas.

## Pontos de acesso

- Botão de medidor ao lado de Configuration, abrindo o painel **Usage limits**.
  Lista todos os agentes configurados, mesmo os ausentes da conversa.
  Indicador visual de aviso/esgotamento e nome acessível equivalente.
- Chips de participantes herdam o estado do agente: badge **Near limit**
  ou percentual a partir de 80%; **Exhausted** quando esgotado.
  Participantes que compartilham agente compartilham a leitura de cota.
- Pills **To:** recebem ícone de alerta, tooltip e descrição acessível.
- Diálogo de participante: nota abaixo do select Agent. Opções recebem
  sufixo curto apenas em aviso/esgotamento. Save permanece habilitado.

## Estrutura de cada agente

Cada linha tem três níveis com peso visual equivalente:

1. Nome do agente, resumo da janela mais crítica e controle de detalhes.
2. Barra/reset quando disponíveis; texto explicativo nos demais casos.
3. **This conversation:** consumo observado, mesmo quando há cota.

Detalhes expandem todas as janelas/buckets, fonte, última leitura e última
tentativa quando houve erro. A janela mais crítica é a de maior estado
de alerta e, dentro dele, maior percentual. Uma cota compartilhada não
pode ser apresentada como cotas independentes por participante/modelo.

## Estados

| Estado | Conteúdo | Tratamento |
| --- | --- | --- |
| Normal | `34% · 5h` e reset | Barra neutra |
| ≥80% | `⚠ Near limit 86% · wk` | Barra/badge com `--warn` |
| ≥100% | `Exhausted` e reset | Barra/badge com `--err` |
| Antes da primeira leitura | `No quota reading yet` | Sem barra; some assim que a leitura termina |
| Indisponível | `Quota not available` e motivo | Sem barra nem trilho |
| Primeira leitura | `Checking…` | Indicador por agente |
| Aviso sem percentual | `Near limit (reported by provider)` | Sem número nem barra |
| Rejeição sem percentual | `Exhausted` | Sem percentual fabricado |
| Erro com leitura anterior | Valor anterior, erro e Retry | Barra com opacidade 0,5; fetched_at preservado |
| Erro sem leitura anterior | `Couldn't read quota` e Retry | Sem barra |
| Janela vencida | Valor anterior e `Reset at…` | Dado desatualizado; consulta automática; nunca zerar localmente |

Erro de leitura e dado vencido não apagam os dados anteriores. Fonte e
horário são por agente; um horário geral não deve sugerir que todos foram
atualizados. Leituras com mais de 15 minutos recebem indicação de idade.

## Reset e idioma

O contrato web usa instantes UTC; o navegador apresenta no fuso local.
Para reset futuro em menos de 24h: `resets in 3h 10m (17:40)`.
Para intervalos maiores: dia/data e horário. Sem reset:
`reset time unknown`. Abreviações: 300 minutos → `5h`; 10080 → `wk`;
outra duração mantém o rótulo informado pelo adapter.

Não prometer que a próxima resposta fornecerá cota. No diálogo de agente
esgotado, usar `Replies may fail`, pois o limite pode afetar um bucket.

## Consumo

Consumo mede execuções desta conversa no aic, não uso total da conta.
Um contador por conversa/agente e referências de contadores cumulativos
ficam no SQLite existente; não há ledger de turnos.

- Campo ausente: `in n/a`, `out n/a` ou `cost n/a`; não fabricar zero.
- Totais parciais: prefixo `≥` nos campos com cobertura incompleta e
  indicação de quantas respostas não informaram uso.
- Nenhuma execução contabilizada: `No usage yet in this conversation`.
- Histórico anterior sem agente registrado: nota única no painel,
  fora dos totais de cada agente.
- Apagar/regenerar resposta não devolve tokens. Copiar/importar histórico
  não cobra as execuções antigas na nova conversa.
- Tooltip: `Spent by aic in this conversation, including regenerated and deleted replies`.
- Se a primeira leitura cumulativa de uma sessão antiga não permite
  separar o gasto atual, iniciar baseline e tratar esse turno como parcial.

## Atualização e cache

O painel abre imediatamente; leituras rodam em paralelo ao uso do chat.
Refresh manual atualiza os adapters habilitados, com indicador por linha.
O botão de refresh fica desabilitado enquanto houver leitura em andamento.
Ao fim de cada turno, atualizar apenas o agente daquela execução, sem
animação adicional. Pedidos concorrentes do mesmo agente são deduplicados
no servidor, inclusive entre abas.

O servidor guarda a última leitura por agente em memória: reload da página
recupera valor e horário anteriores. Cache não sobrevive a reinício do
servidor na v1. Mudança de configuração relevante invalida a leitura.
Leitura vencida tem retentativa limitada; falha não bloqueia o envio.

## Falhas de turno — fase 2

Cartão especial somente com classificação confirmada por campo estruturado:

| Tipo | Título | Ação | Efeito |
| --- | --- | --- | --- |
| quota | `<Agent> usage limit reached` | Change agent… e retry existente | Marca apenas bucket confirmado como esgotado |
| rate_limit | `<Agent> is rate limited` | Try again; Change agent… secundário | Não marca cota esgotada |
| Autenticação/rede/ambíguo | Erro normal | Retry existente | Não altera cota |

Reset aparece apenas se informado; caso contrário, `Reset time not reported
by <Agent>`. Erro original permanece em Details. Change agent… reutiliza
o diálogo do participante, com foco no select Agent.
Na fase 1, erros continuam no cartão normal, preservando o motivo do Codex.

## Acessibilidade e responsividade

- Usar tokens existentes, com texto/ícone além da cor.
- Barra somente com percentual conhecido; `role=meter`, nome, limites,
  valor e `aria-valuetext` incluindo janela/reset.
- Aviso/esgotamento anunciado uma vez por mudança de estado.
- Painel pode ser fechado por botão, clique externo ou Escape, com
  retorno de foco ao botão quando fechado por teclado.
- Em telas ≤700px, painel ocupa a largura disponível do chat.
- Indicadores respeitam `prefers-reduced-motion`.

## Aceite da fase 1

Com Claude, Codex e Gemini configurados, todos aparecem no painel. Codex
exibe as janelas quando disponíveis; Claude/Gemini exibem consumo e estado
explícito. Valores ausentes não aparecem como 0% ou $0.00. Reload preserva
a leitura em cache enquanto o servidor continua ativo. Contagem permanece
correta ao trocar agente, regenerar/apagar resposta e copiar histórico.

## Validação da entrega

A fase 1 passou em 91 testes Rust, 17 testes JavaScript e verificações de
integração HTTP e Chromium. O teste HTTP usa CLIs controladas para conferir
normalização do Codex, isolamento entre conversas, falha sem Usage,
preservação da última leitura e persistência após reinício do servidor.
O navegador verifica os três agentes, cache após reload, 80%/100%, seletor,
envio, regeneração/exclusão, painel estreito e falha de leitura.

O adapter de produção também fez uma consulta autenticada ao Codex
0.160.0 neste sandbox Debian trixie, sem prompt nem geração. Retornou
janelas de 300 e 10080 minutos, com percentual e reset, em cerca de 4,3 s.
Dois turnos reais na mesma sessão confirmaram que `turn.completed` do
`exec resume` é cumulativo; a fase 1 normaliza o segundo evento pelo
baseline persistido da sessão antes de somá-lo à conversa.
Não havia login Claude/agy; suas cotas e falhas reais seguem sem validação.
Não foi possível testar a criação de um container Docker/Podman a partir
deste ambiente; os argumentos do sandbox foram verificados nos testes Rust.

Para repetir as verificações (ajustar o caminho do binário se necessário):

```sh
cargo test
cargo build
AIC_TEST_BINARY="$PWD/target/debug/aic" node --test tests/*.cjs
```

O teste de navegador é opcional quando Playwright/Chromium não estão
instalados; para executá-lo, instalar essas ferramentas e apontar
`AIC_PLAYWRIGHT_MODULE` para o módulo quando estiver fora do repositório.

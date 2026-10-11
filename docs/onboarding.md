# Setup e recuperação

Este guia explica o primeiro uso do aic a partir de um binário publicado. Rust
e edição manual de TOML não são necessários.

## Preparar o ambiente

Execute dentro de um checkout Git:

```sh
aic setup
```

O assistente passa por cinco etapas: ambiente, sandbox, agentes, agente padrão
e conclusão. É possível voltar, sair com `Esc` ou escolher **Finish later**;
as configurações concluídas ficam disponíveis quando `aic setup` for executado
novamente.

O setup não instala CLIs de terceiros. Para cada agente, resolva o diagnóstico
indicado e repita o teste:

```sh
aic doctor
aic agent login claude       # também: gemini, codex ou agy
aic agent test claude
```

O teste de disponibilidade faz uma chamada real mínima e pode consumir cota.
Ele pede confirmação; em automação, use `--yes` conscientemente:

```sh
aic agent test claude --yes --timeout 45
aic setup --agent claude --yes --no-open
```

Apenas um agente pronto é necessário para começar. Uma falha de cota não prova
que o login expirou: aguarde o reset ou selecione outro agente pronto.

## Sandbox

O runtime padrão é Docker e a imagem padrão chama-se `aic-sandbox`:

```sh
aic sandbox build
```

Podman também é aceito. Se não quiser isolamento, selecione `none` no setup ou
execute `aic config set sandbox.runtime none`. Nesse caso, os agentes rodam
diretamente no host.

## Iniciar e validar o chat

```sh
aic chat
# sem abrir o navegador:
aic chat --no-open
```

O painel **Environment** mostra checkout, branch, runtime, imagem e estado de
cada agente. Ele não executa logins nem testes pagos; faça isso no terminal e
depois use **Refresh environment**.

Em uma conversa nova, use **Add participant**, escolha um agente pronto e só
então envie a primeira mensagem. Um agente indisponível nunca é trocado
silenciosamente por outro.

## Diagnóstico rápido

| Sintoma | Ação |
| --- | --- |
| Runtime ausente | Instale/inicie Docker ou Podman, ou escolha `none`. |
| Imagem ausente | Rode `aic sandbox build` e retome `aic setup`. |
| CLI não encontrada | Instale a CLI pelo fornecedor e rode `aic doctor`. |
| Não autenticado | Rode `aic agent login NOME` e depois `aic agent test NOME`. |
| Falha de execução | Repita `aic agent test NOME`; confira a própria CLI. |
| Cota esgotada | Aguarde o reset ou use outro agente pronto. |
| Chat não abre | Use `aic chat --no-open` e abra a URL impressa. |

`aic doctor --json` produz diagnóstico para automação. Os testes ficam em
`.aic/diagnostics/tests.json`, expiram após 24 horas e não armazenam
credenciais, respostas ou stderr do provedor.

## Segurança e dados locais

O chat escuta em `127.0.0.1:7878` por padrão e não tem autenticação. Expor com
`--host 0.0.0.0` permite que qualquer pessoa alcançável pela porta opere os
agentes. O sandbox reduz o alcance de comandos acidentais, mas rede e
credenciais necessárias continuam disponíveis ao agente.

- `~/.config/aic/`: configuração global.
- `.aic/`: planos, histórico do chat, estado e diagnósticos do checkout.
- `~/.local/share/aic/sandbox/`: estado persistente de agentes no sandbox.

O `.aic/` é adicionado ao `.gitignore` pelo `aic init`. Para atualizar o
aic, substitua o binário e execute o setup novamente. Para desinstalar, remova
somente o binário; apague configuração e `.aic/` apenas se também quiser
remover histórico e planos locais.

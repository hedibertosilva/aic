# Documentação

Comece pelo README para uma visão geral e pelo guia de [setup e recuperação](onboarding.md)
para instalar e iniciar o primeiro chat.

## Guias para usuários

- [Setup e recuperação](onboarding.md): instalação, sandbox, diagnóstico e problemas comuns.
- [Worktrees](worktrees.md): como o chat identifica e seleciona checkouts.
- [Planos no chat](chat-plans.md): planos, tarefas, execução e validação.
- [Importação de conversas](chat-import.md): importar e exportar histórico.
- [Compilação a partir do código-fonte](build-from-source.md): desenvolvimento e instalação manual.

## Referências de implementação

Estes documentos descrevem contratos, decisões e validações internas; não são
necessários para o primeiro uso:

- [Contrato de onboarding](onboarding-contract.md)
- [Validação de onboarding](onboarding-validation.md)
- [API de planos no chat](chat-plans-api.md)
- [Backend de planos no chat](chat-plans-backend.md)
- [Validação de planos no chat](chat-plans-validation.md)
- [Especificação de seleção de worktree](worktree-selection-design.md)
- [Spike de prontidão dos agentes](agent-readiness-spike.md)
- [Cotas: descoberta técnica](agent-quotas-spike.md)
- [Cotas: especificação de UX](agent-quotas-ux.md)
- [Notas de release do onboarding](release-notes-onboarding.md)

As referências técnicas podem registrar detalhes de uma versão específica. Para
o comportamento atual do produto, prefira os guias e o código em `src/`.

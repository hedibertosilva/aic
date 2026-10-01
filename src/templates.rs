pub fn plan_template(title: &str) -> String {
    format!(
        "# {title}\n\n\
## Problema\n\n\
<!-- Que problema isso resolve? Para quem? -->\n\n\
## Proposta\n\n\
<!-- O que vai ser feito, em alto nível. -->\n\n\
## Alternativas consideradas\n\n\
<!-- O que mais foi cogitado, e por que não foi escolhido. -->\n\n\
## Impacto\n\n\
<!-- O que muda para usuários, outros times, infraestrutura. -->\n\n\
## Questões abertas\n\n\
<!-- O que ainda não está decidido. -->\n"
    )
}

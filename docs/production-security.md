# Segurança de produção

## Segredos e acesso

- Gere `API_READ_TOKEN` e `API_WRITE_TOKEN` diferentes, com pelo menos 32 caracteres sem espaços.
- Distribua somente `API_READ_TOKEN` para clientes Android. `API_WRITE_TOKEN` deve existir apenas no arquivo de ambiente do servidor e em ferramentas administrativas confiáveis.
- Configure o Environment `production` no GitHub com aprovação obrigatória e acesso ao secret `DEPLOY_SSH_PRIVATE_KEY` apenas para esse Environment.

## Origem Cloudflare

O Caddy remove logs de acesso para não registrar query strings ou cabeçalhos de autorização. A origem ainda precisa de controles fora do repositório:

1. No firewall ou security list da Oracle, permita TCP 80/443 e UDP 443 somente para as faixas IP oficiais da Cloudflare.
2. Configure Cloudflare Authenticated Origin Pulls ou um certificado Cloudflare Origin CA no Caddy antes de bloquear o acesso direto.
3. Configure regras Cloudflare WAF e rate limiting para as rotas de catálogo e documentação.

Não aplique a regra de firewall antes de validar o certificado de origem; isso bloquearia a renovação HTTP-01 atual.

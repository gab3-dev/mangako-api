# Deploy em servidor AMD64

A API, PostgreSQL e as imagens base usadas pelo projeto suportam `linux/arm64`, mas o workflow `.github/workflows/docker.yml` publica somente `linux/amd64`. O workflow `.github/workflows/ci.yml` mantém um smoke test ARM64; para produzir uma imagem ARM64, restaure essa plataforma no workflow de imagem.

## Imagem publicada

Pushes para `master` ou `main` publicam tags de conveniência e um digest imutável:

```text
ghcr.io/OWNER/REPOSITORY@sha256:MANIFEST_DIGEST
```

Tags Git `v*` também geram uma tag correspondente. O deploy automático usa o digest do workflow, nunca uma tag mutável.

## Deploy automático pela CI

Depois de publicar a imagem no branch padrão do repositório, o workflow executa o deploy da API no servidor de produção. O job atualiza somente o serviço `api`; Caddy e PostgreSQL permanecem em execução.

Crie o secret `DEPLOY_SSH_PRIVATE_KEY` no repositório GitHub com o conteúdo da chave privada que acessa o usuário `ubuntu` no servidor. Para a configuração local atual, essa é a chave indicada pelo alias SSH `mangako-api`.

O workflow usa o host `147.15.69.58`, o usuário `ubuntu` e o diretório `/opt/mangako-api`. Ele bloqueia deploys concorrentes e só conclui com sucesso depois que o healthcheck do serviço `api` ficar saudável.

Confirme os manifests publicados:

```sh
docker buildx imagetools inspect ghcr.io/OWNER/REPOSITORY:latest
```

O pacote GHCR precisa estar público ou o servidor deve executar `docker login ghcr.io` antes do pull.

## Execução no servidor AMD64

Crie o arquivo de ambiente de produção a partir de `.env.production.example` e substitua `POSTGRES_PASSWORD`, `API_READ_TOKEN` e `API_WRITE_TOKEN` por valores aleatórios diferentes. O token de escrita fica apenas no servidor. O CI substitui temporariamente `MANGAKO_API_IMAGE` pelo digest publicado durante cada deploy:

```sh
MANGAKO_API_IMAGE='ghcr.io/gab3-dev/mangako-api@sha256:MANIFEST_DIGEST'
```

Baixe as imagens e suba o stack sem recompilar no servidor:

```sh
docker compose --env-file .env.production -f docker-compose.prod.yml pull
docker compose --env-file .env.production -f docker-compose.prod.yml up -d --wait
```

O PostgreSQL fica acessível somente na rede interna do Compose. A API não publica portas no host: apenas o Caddy pode acessá-la como `api:3000` pela rede Docker `edge`.

## HTTPS com Caddy e Cloudflare

O serviço `caddy` usa `deploy/Caddyfile` para publicar `mangako-api.kostudio.io`, redirecionar HTTP para HTTPS e encaminhar as requisições para `api:3000`. Caddy emite e renova automaticamente o certificado da origem. Os volumes `caddy-data` e `caddy-config` preservam certificados e estado entre atualizações dos containers.

No Cloudflare, mantenha o proxy DNS ativo e configure SSL/TLS como `Full (strict)`. As portas TCP `80` e `443` devem aceitar tráfego na origem; UDP `443` é opcional para HTTP/3. A porta `3000` não deve ser exposta publicamente.

Valide o endpoint público:

```sh
curl --fail https://mangako-api.kostudio.io/health
```

Confirme a arquitetura e a saúde:

```sh
docker image inspect "$MANGAKO_API_IMAGE" --format '{{.Architecture}}/{{.Os}}'
curl --fail https://mangako-api.kostudio.io/health
```

O resultado esperado no servidor é `amd64/linux` e `ok`.

## Build nativo alternativo

Se não houver imagem publicada, um host ARM64 pode compilar diretamente:

```sh
export API_READ_TOKEN='replace-with-a-random-read-token-at-least-32-characters'
export API_WRITE_TOKEN='replace-with-a-different-random-write-token-32-chars'
docker compose up -d --build --wait
```

O build Rust pode consumir bastante CPU, memória e espaço em disco. Para produção, prefira a imagem gerada no CI.

## Limite de memória do cache

O cache da API é limitado simultaneamente por quantidade e bytes de body:

```env
CACHE_MAX_ENTRIES=1000
CACHE_MAX_BYTES=67108864
```

O padrão permite até 64 MiB de bodies em cache. Respostas individuais maiores que o orçamento retornam `X-Cache: BYPASS`. Em servidores menores, reduza os valores, por exemplo:

```env
CACHE_MAX_ENTRIES=200
CACHE_MAX_BYTES=16777216
```

## Migração de PostgreSQL AMD64 para ARM64

Não copie diretamente o volume `postgres-data` entre arquiteturas. Use dump lógico e restore.

No servidor antigo:

```sh
docker compose exec -T postgres \
  pg_dump -U mangako -d mangako_api --format=custom \
  > mangako_api.dump
```

Transfira `mangako_api.dump` para o servidor ARM64. No servidor novo, inicie somente um PostgreSQL vazio:

```sh
docker compose up -d postgres --wait
```

Restaure o dump:

```sh
docker compose exec -T postgres \
  pg_restore -U mangako -d mangako_api --clean --if-exists --no-owner \
  < mangako_api.dump
```

Depois inicie a API. Ela aplicará somente migrations ainda pendentes:

```sh
docker compose up -d api --no-build --wait
```

Valide migrations e endpoints antes de remover o servidor antigo.

## Escopo de suporte

O suporte inicial oficial é para:

- `linux/amd64` pela imagem publicada
- `linux/arm64` apenas por build nativo enquanto a publicação multi-arquitetura estiver desativada

ARM 32-bit não faz parte do workflow de teste ou da imagem publicada.

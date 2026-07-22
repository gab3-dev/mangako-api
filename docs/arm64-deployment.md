# Deploy em servidor ARM64

A API, PostgreSQL e todas as imagens base usadas pelo projeto suportam `linux/arm64`. O workflow `.github/workflows/ci.yml` testa o stack em runner ARM64 e, após os testes, chama `.github/workflows/docker.yml` para publicar uma imagem multi-arquitetura no GitHub Container Registry. O workflow de imagem também pode ser executado manualmente pelo GitHub Actions.

## Imagem multi-arquitetura

Pushes para `master` ou `main` publicam:

```text
ghcr.io/OWNER/REPOSITORY:latest
ghcr.io/OWNER/REPOSITORY:master
ghcr.io/OWNER/REPOSITORY:sha-COMMIT
```

Tags Git `v*` também geram uma tag correspondente. O mesmo nome aponta para manifests `linux/amd64` e `linux/arm64`.

Confirme os manifests publicados:

```sh
docker buildx imagetools inspect ghcr.io/OWNER/REPOSITORY:latest
```

O pacote GHCR precisa estar público ou o servidor deve executar `docker login ghcr.io` antes do pull.

## Execução no servidor ARM64

Crie o arquivo de ambiente de produção a partir de `.env.production.example` e substitua `POSTGRES_PASSWORD` e `API_TOKEN` por valores aleatórios. Por padrão, o Compose usa a imagem multi-arquitetura publicada pelo workflow:

```sh
MANGAKO_API_IMAGE='ghcr.io/gab3-dev/mangako-api:latest'
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

O resultado esperado no servidor é `arm64/linux` e `ok`.

## Build nativo alternativo

Se não houver imagem publicada, um host ARM64 pode compilar diretamente:

```sh
export API_TOKEN='replace-with-a-random-token'
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

- `linux/amd64`
- `linux/arm64`

ARM 32-bit não faz parte do workflow de teste ou da imagem publicada.

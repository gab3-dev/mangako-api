# Comandos curl da API

Defina a URL da API, o token de leitura distribuído ao cliente e o token de escrita mantido apenas no servidor:

```sh
export MANGAKO_API_URL='http://localhost:3000'
export MANGAKO_API_READ_TOKEN='replace-with-the-read-token'
export MANGAKO_API_WRITE_TOKEN='replace-with-the-server-only-write-token'
```

## Health check

Rota publica:

```sh
curl --fail-with-body \
  "$MANGAKO_API_URL/health"
```

## Swagger UI

Rota publica. `-L` segue o redirecionamento para a pagina do Swagger:

```sh
curl --fail-with-body --location \
  "$MANGAKO_API_URL/docs"
```

## Especificacao OpenAPI

Rota publica:

```sh
curl --fail-with-body \
  "$MANGAKO_API_URL/api-docs/openapi.json"
```

## Buscar mangas por titulo

Rota protegida por API token:

```sh
curl --fail-with-body --get \
  --header "Authorization: Bearer $MANGAKO_API_READ_TOKEN" \
  --data-urlencode 'title=frieren' \
  --data-urlencode 'limit=6' \
  --data-urlencode 'offset=0' \
  "$MANGAKO_API_URL/mangas"
```

## Listar mangas populares

Sem `title`, a API consulta o MangaDex por quantidade de seguidores e mantém cada página em cache por 6 horas:

```sh
curl --fail-with-body --get \
  --header "Authorization: Bearer $MANGAKO_API_READ_TOKEN" \
  --data-urlencode 'limit=6' \
  --data-urlencode 'offset=0' \
  "$MANGAKO_API_URL/mangas"
```

A variante com barra final tambem e aceita:

```sh
curl --fail-with-body --get \
  --header "Authorization: Bearer $MANGAKO_API_READ_TOKEN" \
  --data-urlencode 'title=frieren' \
  "$MANGAKO_API_URL/mangas/"
```

## Consultar manga

Defina `MANGA_REF` com um UUID interno, slug ou UUID do MangaDex:

```sh
export MANGA_REF='b0b721ff-c388-4486-aa0f-c2b0bb321512'
```

Rota protegida por API token:

```sh
curl --fail-with-body \
  --header "Authorization: Bearer $MANGAKO_API_READ_TOKEN" \
  "$MANGAKO_API_URL/mangas/$MANGA_REF"
```

Para forçar atualização no MangaDex e invalidar o cache desse endpoint:

```sh
curl --fail-with-body --get \
  --header "Authorization: Bearer $MANGAKO_API_WRITE_TOKEN" \
  --data-urlencode 'refresh=true' \
  "$MANGAKO_API_URL/mangas/$MANGA_REF"
```

## Criar manga local

Cria um manga que nao possui registro no MangaDex. Capas gerais aceitam uma URL externa em `sourceUrl` ou uma futura chave interna em `storageKey`; cada capa de volume usa `sourceUrl`.

```sh
curl --fail-with-body --request POST \
  --header "Authorization: Bearer $MANGAKO_API_WRITE_TOKEN" \
  --header 'Content-Type: application/json' \
  --data '{
    "slug": "meu-manga",
    "primaryTitle": "Meu Manga",
    "originalLanguage": "pt-br",
    "localizations": [{
      "language": "pt-br",
      "title": "Meu Manga",
      "description": "Descricao criada localmente",
      "isPrimary": true
    }],
    "covers": [{
      "sourceUrl": "https://example.com/meu-manga.jpg",
      "isPrimary": true
    }]
  }' \
  "$MANGAKO_API_URL/mangas"
```

Use o `id` ou `slug` retornado para criar uma capa geral adicional:

```sh
curl --fail-with-body --request POST \
  --header "Authorization: Bearer $MANGAKO_API_WRITE_TOKEN" \
  --header 'Content-Type: application/json' \
  --data '{"sourceUrl":"https://example.com/nova-capa.jpg","isPrimary":true}' \
  "$MANGAKO_API_URL/mangas/meu-manga/covers"
```

## Criar volume local

```sh
curl --fail-with-body --request POST \
  --header "Authorization: Bearer $MANGAKO_API_WRITE_TOKEN" \
  --header 'Content-Type: application/json' \
  --data '{
    "fileName": "volume-1.jpg",
    "sourceUrl": "https://example.com/volume-1.jpg",
    "volume": "1",
    "locale": "pt-br"
  }' \
  "$MANGAKO_API_URL/mangas/meu-manga/volumes"
```

## Consultar volumes do manga

Rota protegida por API token. Usa o mesmo `MANGA_REF` definido acima:

Sem `locale`, os volumes normais usam japones, com fallback para o idioma original somente se nao houver normais japoneses ativos. Especiais ativos de todos os idiomas entram na mesma ordenacao e paginacao; especiais japoneses nao bloqueiam o fallback. Use `--data-urlencode 'locale=pt'` para normais em portugues (incluindo `pt-br`), ou `locale=original` para o idioma original. Um locale explicito nao faz fallback dos normais. Capas removidas nao sao retornadas.

```sh
curl --fail-with-body --get \
  --header "Authorization: Bearer $MANGAKO_API_READ_TOKEN" \
  --data-urlencode 'limit=50' \
  --data-urlencode 'offset=0' \
  "$MANGAKO_API_URL/mangas/$MANGA_REF/volumes"
```

Para forçar sincronização completa e reconciliação das capas removidas:

```sh
curl --fail-with-body --get \
  --header "Authorization: Bearer $MANGAKO_API_READ_TOKEN" \
  --data-urlencode 'limit=50' \
  --data-urlencode 'offset=0' \
  --data-urlencode 'refresh=true' \
  "$MANGAKO_API_URL/mangas/$MANGA_REF/volumes"
```

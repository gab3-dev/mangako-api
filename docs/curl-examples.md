# Comandos curl da API

Defina a URL da API e o mesmo token configurado em `API_TOKEN`:

```sh
export MANGAKO_API_URL='http://localhost:3000'
export MANGAKO_API_TOKEN='change-me'
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
  --header "Authorization: Bearer $MANGAKO_API_TOKEN" \
  --data-urlencode 'title=frieren' \
  --data-urlencode 'limit=6' \
  --data-urlencode 'offset=0' \
  "$MANGAKO_API_URL/mangas"
```

## Listar mangas populares

Sem `title`, a API consulta o MangaDex por quantidade de seguidores e mantém cada página em cache por 6 horas:

```sh
curl --fail-with-body --get \
  --header "Authorization: Bearer $MANGAKO_API_TOKEN" \
  --data-urlencode 'limit=6' \
  --data-urlencode 'offset=0' \
  "$MANGAKO_API_URL/mangas"
```

A variante com barra final tambem e aceita:

```sh
curl --fail-with-body --get \
  --header "Authorization: Bearer $MANGAKO_API_TOKEN" \
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
  --header "Authorization: Bearer $MANGAKO_API_TOKEN" \
  "$MANGAKO_API_URL/mangas/$MANGA_REF"
```

Para forçar atualização no MangaDex e invalidar o cache desse endpoint:

```sh
curl --fail-with-body --get \
  --header "Authorization: Bearer $MANGAKO_API_TOKEN" \
  --data-urlencode 'refresh=true' \
  "$MANGAKO_API_URL/mangas/$MANGA_REF"
```

## Consultar volumes do manga

Rota protegida por API token. Usa o mesmo `MANGA_REF` definido acima:

```sh
curl --fail-with-body --get \
  --header "Authorization: Bearer $MANGAKO_API_TOKEN" \
  --data-urlencode 'limit=50' \
  --data-urlencode 'offset=0' \
  "$MANGAKO_API_URL/mangas/$MANGA_REF/volumes"
```

Para forçar sincronização completa e reconciliação das capas removidas:

```sh
curl --fail-with-body --get \
  --header "Authorization: Bearer $MANGAKO_API_TOKEN" \
  --data-urlencode 'limit=50' \
  --data-urlencode 'offset=0' \
  --data-urlencode 'refresh=true' \
  "$MANGAKO_API_URL/mangas/$MANGA_REF/volumes"
```

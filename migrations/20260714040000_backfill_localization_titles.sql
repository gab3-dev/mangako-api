UPDATE manga_localizations AS localization
SET title = (
    SELECT manga_aliases.title
    FROM manga_aliases
    WHERE manga_aliases.manga_id = localization.manga_id
      AND lower(manga_aliases.language) = lower(localization.language)
    ORDER BY manga_aliases.title ASC
    LIMIT 1
)
WHERE localization.title IS NULL
  AND EXISTS (
      SELECT 1
      FROM manga_aliases
      WHERE manga_aliases.manga_id = localization.manga_id
        AND lower(manga_aliases.language) = lower(localization.language)
  );

INSERT INTO manga_localizations (manga_id, language, title, description, is_primary)
SELECT DISTINCT ON (alias.manga_id, lower(alias.language))
       alias.manga_id,
       lower(alias.language),
       alias.title,
       NULL,
       false
FROM manga_aliases AS alias
WHERE NOT EXISTS (
    SELECT 1
    FROM manga_localizations AS localization
    WHERE localization.manga_id = alias.manga_id
      AND lower(localization.language) = lower(alias.language)
)
ORDER BY alias.manga_id, lower(alias.language), alias.title ASC;

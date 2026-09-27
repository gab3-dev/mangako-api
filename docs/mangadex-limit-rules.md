A global limit of approximately 5 requests per second per IP address is in effect for api.mangadex.org as a whole

Once above this rate limit, you will receive HTTP 429 responses for all requests to \*.mangadex.org until enough time has passed to bring your rate back in line.

If you persist in sending requests while receiving HTTP 429 responses, our DDoS protection will automatically be triggered and you will be issued a temporary IP address ban, resulting in a generic HTTP 403 response for all requests to \*.mangadex.org. Its duration is not documented and subject to change without notice.

If you still persist in sending requests at that point, we will automatically stop replying to your IP address altogether until we stop receiving traffic from you for a while. This cooldown period is renewed for every request you send while still blocked. Its duration is not documented and subject to change without notice.

Unfortunately, we regularly see particularly abusive request patterns from clients that are just too lazy/disrespectful to use our API decently. If you wonder whether your usage falls in this category, it probably doesn't. And offenders are nearly always well-aware of it.

TL;DR: If you decide to do 500 requests where the same exact feature is implementable with 1 single request, we will drastically throttle you, or block it altogether. The reduced rate and the patterns affected are not documented and subject to change without notice.

source: <https://api.mangadex.org/docs/2-limitations/>

package com.acteon.client;

import com.sun.net.httpserver.HttpServer;
import java.nio.charset.StandardCharsets;

/** Identity exchange used by helpers that validate server-owned response context. */
final class CredentialContextFixture {
    static void install(HttpServer server) { install(server, "prod", "acme"); }
    static void install(HttpServer server, String namespace, String tenant) {
        server.createContext("/v1/auth/identity", exchange -> {
            var bytes = ("{\"credential_id\":\"test\",\"auth_method\":\"api_key\",\"role\":\"operator\",\"principal\":null,\"scope\":{\"namespace\":\""
                + namespace + "\",\"tenant\":\"" + tenant + "\"}}").getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().set("Content-Type", "application/json");
            exchange.sendResponseHeaders(200, bytes.length);
            exchange.getResponseBody().write(bytes);
            exchange.close();
        });
    }
}

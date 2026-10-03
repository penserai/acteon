/** Exercises the actual Java SDK against the simulation's HTTP server. */
import com.acteon.client.ActeonClient;
import com.acteon.client.PlatformOperation;
import com.acteon.client.models.Action;
import com.acteon.client.models.AuditQuery;
import java.util.List;
import java.util.Map;

public class TestJavaClient {
    public static void main(String[] args) throws Exception {
        String url = System.getenv().getOrDefault("ACTEON_URL", "http://localhost:8080");
        try (var client = new ActeonClient(url)) {
            if (!client.health()) throw new AssertionError("health");
            if (!client.platformRequest(PlatformOperation.HEALTH_HEALTH, null, null, null)
                    .get("status").asText().equals("ok")) throw new AssertionError("platform health");
            var action = new Action("test", "java-client", "email", "send_notification", Map.of("to", "test@example.com"));
            if (!client.dispatch(action).isExecuted()) throw new AssertionError("dispatch");
            var batch = client.dispatchBatch(List.of(
                new Action("test", "java-client", "email", "send_notification", Map.of()),
                new Action("test", "java-client", "email", "send_notification", Map.of())));
            if (batch.size() != 2) throw new AssertionError("batch");
            client.listRules();
            client.queryAudit(new AuditQuery());
            System.out.println("Java SDK: health, platform request, dispatch, batch, rules, audit passed");
        }
    }
}

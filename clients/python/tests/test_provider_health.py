"""Tests for provider health models in acteon_client.models."""

import unittest
from acteon_client.models import ProviderHealthStatus, ListProviderHealthResponse


class TestProviderHealthStatus(unittest.TestCase):
    """Tests for the ProviderHealthStatus dataclass."""

    def test_from_dict_complete(self):
        """ProviderHealthStatus.from_dict() should parse all fields."""
        data = {
            "provider": "email",
            "healthy": True,
            "health_check_error": None,
            "circuit_breaker_state": "closed",
            "total_requests": 1500,
            "successes": 1480,
            "failures": 20,
            "success_rate": 98.67,
            "avg_latency_ms": 45.2,
            "p50_latency_ms": 32.0,
            "p95_latency_ms": 120.5,
            "p99_latency_ms": 250.0,
            "last_request_at": 1707900000000,
            "last_error": "connection timeout",
        }
        status = ProviderHealthStatus.from_dict(data)
        self.assertEqual(status.provider, "email")
        self.assertTrue(status.healthy)
        self.assertIsNone(status.health_check_error)
        self.assertEqual(status.circuit_breaker_state, "closed")
        self.assertEqual(status.total_requests, 1500)
        self.assertEqual(status.successes, 1480)
        self.assertEqual(status.failures, 20)
        self.assertEqual(status.success_rate, 98.67)
        self.assertEqual(status.avg_latency_ms, 45.2)
        self.assertEqual(status.p50_latency_ms, 32.0)
        self.assertEqual(status.p95_latency_ms, 120.5)
        self.assertEqual(status.p99_latency_ms, 250.0)
        self.assertEqual(status.last_request_at, 1707900000000)
        self.assertEqual(status.last_error, "connection timeout")

    def test_from_dict_minimal(self):
        """ProviderHealthStatus.from_dict() should handle optional fields."""
        data = {
            "provider": "sms",
            "healthy": False,
            "circuit_breaker_state": "open",
            "total_requests": 100,
            "successes": 50,
            "failures": 50,
            "success_rate": 50.0,
            "avg_latency_ms": 100.0,
            "p50_latency_ms": 90.0,
            "p95_latency_ms": 200.0,
            "p99_latency_ms": 300.0,
        }
        status = ProviderHealthStatus.from_dict(data)
        self.assertEqual(status.provider, "sms")
        self.assertFalse(status.healthy)
        self.assertIsNone(status.health_check_error)
        self.assertIsNone(status.last_request_at)
        self.assertIsNone(status.last_error)


class TestListProviderHealthResponse(unittest.TestCase):
    """Tests for the ListProviderHealthResponse dataclass."""

    def test_from_dict_empty(self):
        """ListProviderHealthResponse.from_dict() should handle empty provider list."""
        data = {"providers": []}
        response = ListProviderHealthResponse.from_dict(data)
        self.assertEqual(response.providers, [])

    def test_from_dict_multiple_providers(self):
        """ListProviderHealthResponse.from_dict() should parse multiple providers."""
        data = {
            "providers": [
                {
                    "provider": "email",
                    "healthy": True,
                    "circuit_breaker_state": "closed",
                    "total_requests": 1000,
                    "successes": 990,
                    "failures": 10,
                    "success_rate": 99.0,
                    "avg_latency_ms": 50.0,
                    "p50_latency_ms": 40.0,
                    "p95_latency_ms": 100.0,
                    "p99_latency_ms": 150.0,
                },
                {
                    "provider": "slack",
                    "healthy": False,
                    "health_check_error": "connection refused",
                    "circuit_breaker_state": "open",
                    "total_requests": 500,
                    "successes": 450,
                    "failures": 50,
                    "success_rate": 90.0,
                    "avg_latency_ms": 200.0,
                    "p50_latency_ms": 150.0,
                    "p95_latency_ms": 400.0,
                    "p99_latency_ms": 600.0,
                    "last_error": "timeout",
                },
            ]
        }
        response = ListProviderHealthResponse.from_dict(data)
        self.assertEqual(len(response.providers), 2)
        self.assertEqual(response.providers[0].provider, "email")
        self.assertTrue(response.providers[0].healthy)
        self.assertEqual(response.providers[1].provider, "slack")
        self.assertFalse(response.providers[1].healthy)
        self.assertEqual(response.providers[1].health_check_error, "connection refused")


if __name__ == "__main__":
    unittest.main()

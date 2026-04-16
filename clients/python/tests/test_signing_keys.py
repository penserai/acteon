"""Tests for the JWKS-style signing key discovery helpers."""

import unittest

from acteon_client.models import SigningKeyEntry, SigningKeysResponse


class TestSigningKeyEntry(unittest.TestCase):
    def test_from_dict_complete(self):
        data = {
            "signer_id": "ci-bot",
            "kid": "k1",
            "algorithm": "Ed25519",
            "public_key": "LZkUda4pibD+v4yfHrLyw9Dnt7OLa6PGzSRGOcN1c4o=",
            "tenants": ["acme", "globex"],
            "namespaces": ["prod", "staging"],
        }
        entry = SigningKeyEntry.from_dict(data)
        self.assertEqual(entry.signer_id, "ci-bot")
        self.assertEqual(entry.kid, "k1")
        self.assertEqual(entry.algorithm, "Ed25519")
        self.assertEqual(entry.public_key, data["public_key"])
        self.assertEqual(entry.tenants, ["acme", "globex"])
        self.assertEqual(entry.namespaces, ["prod", "staging"])

    def test_from_dict_missing_scope_defaults_to_empty_lists(self):
        # Defensive: if the server ever omits tenants/namespaces,
        # don't explode. An empty list is distinguishable from a
        # wildcard ["*"] so callers can tell them apart.
        data = {
            "signer_id": "deploy-svc",
            "kid": "k0",
            "algorithm": "Ed25519",
            "public_key": "AAAA",
        }
        entry = SigningKeyEntry.from_dict(data)
        self.assertEqual(entry.tenants, [])
        self.assertEqual(entry.namespaces, [])


class TestSigningKeysResponse(unittest.TestCase):
    def test_from_dict_with_keys(self):
        data = {
            "keys": [
                {
                    "signer_id": "ci-bot",
                    "kid": "k1",
                    "algorithm": "Ed25519",
                    "public_key": "AAAA",
                    "tenants": ["*"],
                    "namespaces": ["*"],
                },
                {
                    "signer_id": "ci-bot",
                    "kid": "k2",
                    "algorithm": "Ed25519",
                    "public_key": "BBBB",
                    "tenants": ["*"],
                    "namespaces": ["*"],
                },
            ],
            "count": 2,
        }
        resp = SigningKeysResponse.from_dict(data)
        self.assertEqual(resp.count, 2)
        self.assertEqual(len(resp.keys), 2)
        self.assertEqual(resp.keys[0].kid, "k1")
        self.assertEqual(resp.keys[1].kid, "k2")

    def test_from_dict_empty_when_signing_disabled(self):
        # Server-side shape when [signing].enabled is false: empty
        # keys array with count 0. The wrapper should round-trip
        # cleanly rather than requiring callers to handle a missing
        # "keys" field specially.
        resp = SigningKeysResponse.from_dict({"keys": [], "count": 0})
        self.assertEqual(resp.count, 0)
        self.assertEqual(resp.keys, [])

    def test_from_dict_derives_count_when_missing(self):
        # A defensive fallback: if an older/custom server doesn't
        # emit `count`, use the length of `keys` so the struct
        # remains self-consistent. The server always emits count
        # today, but mirroring it as authoritative would leave the
        # struct fragile to a minor server change.
        resp = SigningKeysResponse.from_dict({"keys": []})
        self.assertEqual(resp.count, 0)


if __name__ == "__main__":
    unittest.main()

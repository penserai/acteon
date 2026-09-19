import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

from container_evidence import verify


class EvidenceTests(unittest.TestCase):
    def fixture(self, directory, *, missing=False, wrong_subject=False, wrong_image=False, corrupt=False, wrong_layer=False):
        files = {}

        def blob(value, media="application/vnd.oci.image.manifest.v1+json"):
            raw = json.dumps(value).encode()
            digest = hashlib.sha256(raw).hexdigest()
            files["blobs/sha256/" + digest] = raw
            return {"digest": "sha256:" + digest, "size": len(raw), "mediaType": media}

        layer_data = b"example uncompressed layer"
        config = blob({"rootfs": {"diff_ids": ["sha256:" + hashlib.sha256(layer_data).hexdigest()]}})
        image = blob({"config": config, "layers": []})
        subjects = [{"digest": {"sha256": "bad" if wrong_subject else image["digest"][7:]}}]
        statements = [("https://spdx.dev/Document", {"spdxVersion": "SPDX-2.3", "packages": [{}]})]
        if not missing:
            statements.append(("https://slsa.dev/provenance/v0.2", {"buildType": "https://mobyproject.org/buildkit@v1"}))
        layers = [blob({"subject": subjects, "predicateType": kind, "predicate": predicate},
                       "application/vnd.in-toto+json") for kind, predicate in statements]
        attestation = blob({"layers": layers})
        index = blob({"manifests": [image, attestation]})
        files["index.json"] = json.dumps({"manifests": [index]}).encode()
        config_raw = files["blobs/sha256/" + config["digest"][7:]]
        if corrupt:
            files["blobs/sha256/" + image["digest"][7:]] = b"{}"

        def write_archive(name, members):
            path = directory / name
            with tarfile.open(path, "w") as archive:
                for member_name, raw in members.items():
                    info = tarfile.TarInfo(member_name)
                    info.size = len(raw)
                    archive.addfile(info, io.BytesIO(raw))
            return path

        return (write_archive("oci.tar", files), write_archive("docker.tar", {
            "manifest.json": b'[{"Config":"config.json","Layers":["layer.tar"]}]',
            "config.json": b'{}' if wrong_image else config_raw,
            "layer.tar": b"tampered" if wrong_layer else layer_data,
        }))

    def test_matching_exports(self):
        with tempfile.TemporaryDirectory() as directory:
            evidence = verify(*self.fixture(Path(directory)))
            self.assertEqual(set(evidence), {"sbom.spdx.json", "provenance.json", "identity.json"})

    def test_rejects_incomplete_or_mismatched_evidence(self):
        for mutation in ("missing", "wrong_subject", "wrong_image", "corrupt", "wrong_layer"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                with self.assertRaises(ValueError):
                    verify(*self.fixture(Path(directory), **{mutation: True}))


if __name__ == "__main__":
    unittest.main()

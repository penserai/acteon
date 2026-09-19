#!/usr/bin/env python3
"""Validate BuildKit attestations and bind them to the Docker archive we scan."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import tarfile


def read_json(archive, name):
    member = archive.getmember(name)
    if not member.isfile() or member.size > 64 * 1024 * 1024:
        raise ValueError(f"Invalid metadata member: {name}")
    with archive.extractfile(member) as stream:
        return stream.read()


def verify(oci_path, docker_path):
    with tarfile.open(oci_path) as archive:
        def blob(descriptor):
            digest = descriptor["digest"]
            if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
                raise ValueError("Expected SHA-256 descriptor")
            raw = read_json(archive, "blobs/sha256/" + digest.split(":")[1])
            if len(raw) != descriptor["size"] or hashlib.sha256(raw).hexdigest() != digest[7:]:
                raise ValueError("Metadata digest/size mismatch")
            return json.loads(raw)

        images, statements = {}, []

        def visit(descriptor):
            manifest = blob(descriptor)
            if "manifests" in manifest:
                for child in manifest["manifests"]:
                    visit(child)
            elif any(layer["mediaType"] == "application/vnd.in-toto+json"
                     for layer in manifest["layers"]):
                statements.extend(blob(layer) for layer in manifest["layers"]
                                  if layer["mediaType"] == "application/vnd.in-toto+json")
            else:
                blob(manifest["config"])
                images[descriptor["digest"][7:]] = manifest["config"]["digest"][7:]

        for descriptor in json.loads(read_json(archive, "index.json"))["manifests"]:
            visit(descriptor)
        if len(images) != 1:
            raise ValueError("Expected exactly one platform image")
        image_digest, config_digest = next(iter(images.items()))
        evidence = {}
        for statement in statements:
            subjects = statement.get("subject", [])
            if not subjects or any(s.get("digest", {}).get("sha256") != image_digest for s in subjects):
                raise ValueError("Attestation subject does not match image")
            kind = statement["predicateType"]
            predicate = statement["predicate"]
            if kind == "https://spdx.dev/Document":
                if not predicate.get("spdxVersion") or not predicate.get("packages"):
                    raise ValueError("Empty SPDX inventory")
                evidence["sbom.spdx.json"] = predicate
            elif kind.startswith("https://slsa.dev/provenance/"):
                if not (predicate.get("buildType") or predicate.get("buildDefinition")):
                    raise ValueError("Missing provenance build definition")
                evidence["provenance.json"] = statement
        if set(evidence) != {"sbom.spdx.json", "provenance.json"}:
            raise ValueError("Both SBOM and provenance are required")

    with tarfile.open(docker_path) as archive:
        manifests = json.loads(read_json(archive, "manifest.json"))
        if len(manifests) != 1:
            raise ValueError("Expected one Docker image")
        raw = read_json(archive, manifests[0]["Config"])
        if hashlib.sha256(raw).hexdigest() != config_digest:
            raise ValueError("Scanned image differs from attested image")
        diff_ids = json.loads(raw)["rootfs"]["diff_ids"]
        layers = manifests[0]["Layers"]
        if len(layers) != len(diff_ids):
            raise ValueError("Layer count differs from attested config")
        for layer, expected in zip(layers, diff_ids):
            member = archive.getmember(layer)
            if not member.isfile():
                raise ValueError("Layer must be a regular file")
            digest = hashlib.sha256()
            with archive.extractfile(member) as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(chunk)
            if "sha256:" + digest.hexdigest() != expected:
                raise ValueError("Scanned filesystem layer differs from attested config")
    evidence["identity.json"] = {"manifest_sha256": image_digest, "config_sha256": config_digest}
    return evidence


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--oci", required=True, type=Path)
    parser.add_argument("--docker", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    evidence = verify(args.oci, args.docker)
    args.output.mkdir(parents=True, exist_ok=True)
    for name, document in evidence.items():
        (args.output / name).write_text(json.dumps(document, indent=2) + "\n")
    print("Verified SBOM, provenance subjects, and matching image config")

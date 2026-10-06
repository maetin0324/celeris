#!/usr/bin/env python3
"""Loopback-only RouteLLM /estimate v1 adapter; never calls model generation APIs."""

import argparse
import json
import math
import os
from pathlib import Path
import re
import signal
from http.server import BaseHTTPRequestHandler, HTTPServer


ESTIMATOR_ID = "routellm-bert"
VERSION = "1"
MAX_BODY = 1024 * 1024
IDENTITY = re.compile(r"^[A-Za-z0-9._:/-]{1,128}$")
DEPENDENCIES = {"needs_network": False, "external_embeddings": False}


def load_classifier(router, weights_dir, fake):
    if fake:
        return lambda prompt: 0.75
    if router != "bert":
        raise ValueError("only bert is supported: other routers may use external embeddings")
    checkpoint = Path(weights_dir).expanduser().resolve(strict=True)
    if not checkpoint.is_dir():
        raise ValueError("weights-dir must be a local directory")
    # Set before importing any RouteLLM/Hugging Face module. Never fetch weights.
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["HF_DATASETS_OFFLINE"] = "1"
    # RouteLLM imports its unused similarity router, which constructs OpenAI().
    # Give it an inert key so no ambient production credential enters this process.
    os.environ["OPENAI_API_KEY"] = "disabled-route-llm-wrapper"
    from routellm.routers.routers import BERTRouter  # pylint: disable=import-outside-toplevel

    classifier = BERTRouter(checkpoint_path=str(checkpoint))
    return classifier.calculate_strong_win_rate


def estimate(request, strong, weak, classifier):
    if not isinstance(request, dict) or not isinstance(request.get("request_id"), str):
        raise ValueError("invalid request_id")
    request_id = request["request_id"]
    if not IDENTITY.fullmatch(request_id):
        raise ValueError("invalid request_id")
    candidates = request.get("candidates")
    if not isinstance(candidates, list) or not 1 <= len(candidates) <= 128:
        raise ValueError("invalid candidates")
    ids = []
    for candidate in candidates:
        if not isinstance(candidate, dict):
            raise ValueError("invalid candidate")
        model_id = candidate.get("model_profile_id")
        if not isinstance(model_id, str) or not IDENTITY.fullmatch(model_id):
            raise ValueError("invalid model_profile_id")
        ids.append(model_id)
    if len(ids) != len(set(ids)):
        raise ValueError("duplicate model_profile_id")
    if not isinstance(request.get("context_features"), dict):
        raise ValueError("invalid context_features")
    prompt = request.get("optional_prompt")
    if prompt is not None and not isinstance(prompt, str):
        raise ValueError("invalid optional_prompt")

    pair_present = strong in ids and weak in ids
    score = None
    if pair_present and prompt:
        score = float(classifier(prompt))
        if not math.isfinite(score) or not 0 <= score <= 1:
            raise ValueError("classifier returned invalid win-rate")

    estimates = []
    for model_id in ids:
        if model_id not in (strong, weak):
            reasons = ["quality_unknown", "outside_configured_pair"]
        elif not pair_present:
            reasons = ["quality_unknown", "pair_incomplete"]
        elif score is None:
            reasons = ["quality_unknown", "prompt_required"]
        elif model_id == strong:
            # RouteLLM's pair win-rate is not calibrated to Celeris quality.
            reasons = ["uncalibrated_pair_score", f"raw_pair_win_rate={score:.9g}"]
        else:
            reasons = ["uncalibrated_pair_score", "raw_pair_score_on_strong"]
        estimates.append({"model_profile_id": model_id, "index": None,
                          "confidence": None, "reasons": reasons})
    return {"request_id": request_id, "estimator_id": ESTIMATOR_ID,
            "version": VERSION, "estimates": estimates,
            "dependencies": DEPENDENCIES}


def handler_for(strong, weak, classifier):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, _format, *_args):
            # Do not log prompts or request bodies.
            pass

        def send_json(self, status, body):
            data = json.dumps(body, allow_nan=False, separators=(",", ":")).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path != "/healthz":
                self.send_json(404, {"error": "not_found"})
                return
            self.send_json(200, {"status": "ready", "estimator_id": ESTIMATOR_ID,
                                 "version": VERSION, "protocol_version": 1,
                                 "needs_prompt": True, "dependencies": DEPENDENCIES})

        def do_POST(self):
            if self.path != "/estimate":
                self.send_json(404, {"error": "not_found"})
                return
            try:
                length = int(self.headers.get("Content-Length", "-1"))
                if not 0 <= length <= MAX_BODY:
                    raise ValueError("invalid body size")
                request = json.loads(self.rfile.read(length))
                response = estimate(request, strong, weak, classifier)
            except (ValueError, TypeError, UnicodeDecodeError, json.JSONDecodeError) as error:
                self.send_json(400, {"error": str(error)})
                return
            except Exception:
                self.send_json(503, {"error": "classifier_unavailable"})
                return
            self.send_json(200, response)

    return Handler


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1", choices=["127.0.0.1"])
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--router", required=True, choices=["bert"])
    parser.add_argument("--weights-dir", required=True)
    parser.add_argument("--strong", required=True)
    parser.add_argument("--weak", required=True)
    parser.add_argument("--fake-classifier", action="store_true")
    args = parser.parse_args()
    if not 0 <= args.port <= 65535:
        parser.error("port must be between 0 and 65535")
    if args.strong == args.weak or not all(IDENTITY.fullmatch(i) for i in (args.strong, args.weak)):
        parser.error("strong and weak must be distinct valid model_profile_id values")
    try:
        classifier = load_classifier(args.router, args.weights_dir, args.fake_classifier)
    except (OSError, ValueError, ImportError) as error:
        parser.error(str(error))

    server = HTTPServer((args.host, args.port), handler_for(args.strong, args.weak, classifier))

    def stop(_signum, _frame):
        raise SystemExit(0)

    signal.signal(signal.SIGTERM, stop)
    print(f"READY port={server.server_port}", flush=True)
    try:
        server.serve_forever(poll_interval=0.1)
    finally:
        server.server_close()


if __name__ == "__main__":
    main()

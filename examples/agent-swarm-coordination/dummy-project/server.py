"""
Minimal Flask server -- starting point for vibe coding with Claude Code.

This is a bare-bones web server that you'll extend during the coding session.
Ask Claude Code to add endpoints, middleware, database connections, etc.
"""

from flask import Flask, jsonify

app = Flask(__name__)


@app.route("/")
def index():
    return jsonify({"message": "Hello from the dummy project!"})


if __name__ == "__main__":
    app.run(host="0.0.0.0", port=5000, debug=True)

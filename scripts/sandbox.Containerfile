# The image code_exec uses for Python on this machine: the official slim
# image plus the usual data-analysis libraries, since sandbox containers
# have no network to pip install. Built by scripts/sandbox-image.sh;
# gai.toml points [sandbox.images] python at it.
FROM docker.io/library/python:3.13-slim

RUN pip install --no-cache-dir numpy pandas matplotlib openpyxl tabulate

# Model servers

gai can run open models itself: it starts **llama.cpp**, **vLLM** or
**Ollama** in a container, checks that the server answers, and adds its
models to the catalog. Start, stop and watch them under **Settings › AI
models › Model servers**, with the machine's GPUs shown above them.

It needs Docker or Podman where gai runs (the same engine as the code
sandbox). Model servers belong to the default organization and are managed
by its admins.

## Deploying one

```yaml
name: gemma
server: llamacpp                     # llamacpp | vllm | ollama
model: ggml-org/gemma-3-1b-it-GGUF   # see below
gpu: false                           # all NVIDIA GPUs when true
context: 8192                        # context window, optional
port: 8081                           # host port; a free one when left out
tools: true                          # whether its models can call tools
env: { HF_TOKEN: "${HF_TOKEN}" }     # ${VAR} comes from gai's environment
```

| Server | `model` | Notes |
|---|---|---|
| `llamacpp` | A Hugging Face GGUF repository (`ggml-org/gemma-3-1b-it-GGUF`, add `:Q4_K_M` for a quantization) or a URL to a `.gguf` file | CPU or GPU. Runs with `--jinja` so models can call tools. Image `ghcr.io/ggml-org/llama.cpp:server` (`server-cuda` with `gpu`) |
| `vllm` | A Hugging Face model id (`Qwen/Qwen3-8B`) | Needs an NVIDIA GPU. With `tools: true` it turns on vLLM's tool parser (`tool_parser`, default `hermes`). Image `vllm/vllm-openai` |
| `ollama` | Models to pull, separated by commas (`qwen3:4b`), or nothing | Pulled once the server is up. Image `ollama/ollama` |

`image` picks another image and `args` adds command-line arguments.

Once it answers, its models are `deploy-<name>/<model>` everywhere a model
is chosen, for example `deploy-gemma/ggml-org/gemma-3-1b-it-GGUF`. The
first start downloads the image and the model, which can take a while;
the deployment shows **Starting** until then, and **Logs** shows the
server's output. Downloaded weights live in a volume per server kind
(`gai-models-llamacpp`, ...), so removing a deployment and adding it again
doesn't download them again. Containers restart with the engine unless
they were stopped.

## GPUs

GPU status comes from `nvidia-smi` on the machine gai runs on: each GPU's
memory, use and temperature. Containers get GPUs through the engine, which
needs the [NVIDIA Container
Toolkit](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html);
the page says when the engine can't use them.

## Settings

```toml
[deploy]
engine = "unix:///run/podman/podman.sock"  # default: [sandbox] engine, DOCKER_HOST, then the usual sockets
bind = "127.0.0.1"                          # the host address servers are published on
host = "127.0.0.1"                          # how gai reaches them
```

When gai itself runs in a container (as `scripts/serve.sh` does), set
`host = "host.containers.internal"` (Podman) or `host.docker.internal`
(Docker), and `bind = "0.0.0.0"` so the published port is reachable from
there. Anything that can reach that port can use the model, so keep it off
public networks.

## API

| Method | Path | |
|---|---|---|
| GET | `/v1/deployments` | Each server: spec, container state, health and models |
| POST | `/v1/deployments` | Save one (JSON or `{"yaml": "..."}`) and start it. Admin |
| GET | `/v1/deployments/gpus` | GPUs and whether the engine can use them |
| GET, DELETE | `/v1/deployments/{name}` | One server; delete stops it and removes its container. Admin |
| POST | `/v1/deployments/{name}/start`, `/stop` | Admin |
| GET | `/v1/deployments/{name}/logs?lines=200` | Its output. Admin |

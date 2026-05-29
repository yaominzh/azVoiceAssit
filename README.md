# azVoiceAssist — P0

Always-listening voice loop: listen → transcribe → refine → speak.

## Setup (once)

```bash
brew install portaudio
/opt/homebrew/bin/python3.12 -m venv .venv
source .venv/bin/activate
pip install --upgrade pip
pip install -r requirements.txt
export OMLX_API_KEY=rdaz1234        # oMLX must be running on :8002
```

First run downloads `whisper-base-mlx` (~150 MB). macOS will prompt for mic permission.

## Run

```bash
python assistant.py            # live mic loop; speak, pause, hear it refined
python assistant.py --once "um so like the meetin is uh tomorrow"   # headless, no mic
```

## Test

```bash
pytest -v
```

@echo off
rem Windows twin of download_dependencies.sh: fetch the ML models the PDF/image
rem pipeline needs into .models\ and pdfium.dll into .pdfium\lib\, so a native
rem (non-WSL) build runs out of the box:
rem
rem     cargo build --release -p docling-cli
rem     target\release\docling-rs paper.pdf
rem
rem Everything lands relative to the repo root (the binary resolves .models\ and
rem .pdfium\lib next to the CWD or the executable, no env vars needed). The
rem models come from the same GitHub release the Linux script uses; pdfium.dll
rem comes straight from pdfium-binaries (the release only hosts the Linux .so).
rem
rem Usage: scripts\install\download_dependencies.bat [--no-asr] [--no-int8] [--force]
rem Needs curl.exe and tar.exe (both ship with Windows 10 1803+).
setlocal enabledelayedexpansion

set "BASE_URL=https://github.com/docling-project/docling.rs/releases/download/models-v1"
if not "%DOCLING_RS_MODELS_URL%"=="" set "BASE_URL=%DOCLING_RS_MODELS_URL%"
set "ASR_BASE_URL=https://huggingface.co/onnx-community/whisper-tiny/resolve/main"
set "OCR_EN_URL=https://huggingface.co/SWHL/RapidOCR/resolve/main/PP-OCRv3/en_PP-OCRv3_rec_infer.onnx"
set "EN_DICT_URL=https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/main/ppocr/utils/en_dict.txt"
set "PDFIUM_URL=https://github.com/bblanchon/pdfium-binaries/releases/latest/download/pdfium-win-x64.tgz"

set WITH_ASR=1
set WITH_INT8=1
set FORCE=0
:parse
if "%~1"=="" goto parsed
if "%~1"=="--no-asr"  set WITH_ASR=0
if "%~1"=="--no-int8" set WITH_INT8=0
if "%~1"=="--force"   set FORCE=1
if "%~1"=="--help" (
  echo usage: download_dependencies.bat [--no-asr] [--no-int8] [--force]
  exit /b 0
)
shift
goto parse
:parsed

rem repo root = two levels above this script
cd /d "%~dp0..\.."

where curl >nul 2>nul || (echo error: curl.exe not found ^(ships with Windows 10 1803+^) & exit /b 1)

rem Never hang forever on a dead mirror: cap the connect phase, abort a
rem transfer stalled below 1 KiB/s for a minute, retry transient failures
rem (docling proper added the same guard, issue #3784). No --retry-delay: a
rem fixed delay bunches every attempt into a few seconds, which is useless
rem against the per-IP 429 the third-party hosts answer shared CI/office
rem egress with - curl's default doubling spreads them out (as in the .sh).
set "CURL_TIMEOUTS=--connect-timeout 30 --speed-limit 1024 --speed-time 60 --retry 5"

if not exist .models\tableformer mkdir .models\tableformer
if not exist .models\asr mkdir .models\asr
if not exist .models\chunk mkdir .models\chunk
if not exist .pdfium\lib mkdir .pdfium\lib

echo fetching docling.rs ML dependencies from %BASE_URL%

rem --- pdfium (native DLL, not re-hosted in the models release) --------------
rem No "if exist A if COND (...) else (...)" here: in cmd the else binds to the
rem INNER if, so when the file is missing neither branch runs at all.
if %FORCE%==1 goto :pdfium_dl
if not exist .pdfium\lib\pdfium.dll goto :pdfium_dl
echo   = .pdfium\lib\pdfium.dll (already present)
goto :pdfium_done
:pdfium_dl
echo   ^> .pdfium\lib\pdfium.dll
curl -fsSL %CURL_TIMEOUTS% -o "%TEMP%\pdfium-win-x64.tgz" "%PDFIUM_URL%" || goto :fail
tar -xzf "%TEMP%\pdfium-win-x64.tgz" -C .pdfium bin/pdfium.dll || goto :fail
move /y .pdfium\bin\pdfium.dll .pdfium\lib\pdfium.dll >nul
rmdir .pdfium\bin 2>nul
del "%TEMP%\pdfium-win-x64.tgz" 2>nul
:pdfium_done

rem --- required models --------------------------------------------------------
call :fetch "%BASE_URL%/layout_heron.onnx"              .models\layout_heron.onnx           || goto :fail
call :fetch "%BASE_URL%/ocr_rec.onnx"                   .models\ocr_rec.onnx                || goto :fail
call :fetch "%BASE_URL%/ppocr_keys_v1.txt"              .models\ppocr_keys_v1.txt           || goto :fail
rem English PP-OCRv3 pair - the runtime *default* OCR model (the ch_ pair
rem above is the conformance one). Release first, upstream second: the mirror
rem only exists on tags published after it was added, and both hosts answer
rem 429 under load. Optional either way - the pipeline degrades to the ch_
rem model with a warning when the pair is missing.
call :fetch_opt "%BASE_URL%/ocr_rec_en.onnx"            .models\ocr_rec_en.onnx
if not exist .models\ocr_rec_en.onnx call :fetch_opt "%OCR_EN_URL%" .models\ocr_rec_en.onnx
call :fetch_opt "%BASE_URL%/en_dict.txt"                .models\en_dict.txt
if not exist .models\en_dict.txt call :fetch_opt "%EN_DICT_URL%" .models\en_dict.txt
call :fetch "%BASE_URL%/encoder.onnx"                   .models\tableformer\encoder.onnx    || goto :fail
call :fetch_opt "%BASE_URL%/encoder.onnx.data"          .models\tableformer\encoder.onnx.data
call :fetch "%BASE_URL%/decoder.onnx"                   .models\tableformer\decoder.onnx    || goto :fail
call :fetch_opt "%BASE_URL%/decoder.onnx.data"          .models\tableformer\decoder.onnx.data
rem True-KV-cache decoder - preferred by the Rust loop when present.
call :fetch_opt "%BASE_URL%/decoder_kv.onnx"            .models\tableformer\decoder_kv.onnx
call :fetch_opt "%BASE_URL%/decoder_kv.onnx.data"       .models\tableformer\decoder_kv.onnx.data
call :fetch "%BASE_URL%/bbox.onnx"                      .models\tableformer\bbox.onnx       || goto :fail
call :fetch_opt "%BASE_URL%/bbox.onnx.data"             .models\tableformer\bbox.onnx.data
call :fetch_opt "%BASE_URL%/wordmap.json"               .models\tableformer\wordmap.json
call :fetch_opt "%BASE_URL%/picture_classifier.onnx"    .models\picture_classifier.onnx
call :fetch_opt "%BASE_URL%/chunk_tokenizer.json"       .models\chunk\tokenizer.json

rem --- ASR (Whisper tiny) -----------------------------------------------------
rem Release mirror (asr_*) first, Hugging Face second - same order as the .sh
rem twin, so a rate-limited HF doesn't sink the install. A goto guard rather
rem than an "if (...)" block, and each tier on its own line: cmd binds a "||"
rem written after an "if" to the whole if-statement, so a skipped fallback
rem would inherit a stale errorlevel and jump to :fail. The required files
rem are guarded by their own "if not exist ... goto :fail" instead.
if %WITH_ASR%==0 goto :asr_done
call :fetch_opt "%BASE_URL%/asr_encoder_model.onnx"     .models\asr\encoder_model.onnx
if not exist .models\asr\encoder_model.onnx call :fetch "%ASR_BASE_URL%/onnx/encoder_model.onnx" .models\asr\encoder_model.onnx
if not exist .models\asr\encoder_model.onnx goto :fail
call :fetch_opt "%BASE_URL%/asr_decoder_model.onnx"     .models\asr\decoder_model.onnx
if not exist .models\asr\decoder_model.onnx call :fetch "%ASR_BASE_URL%/onnx/decoder_model.onnx" .models\asr\decoder_model.onnx
if not exist .models\asr\decoder_model.onnx goto :fail
call :fetch_opt "%BASE_URL%/asr_vocab.json"             .models\asr\vocab.json
if not exist .models\asr\vocab.json call :fetch "%ASR_BASE_URL%/vocab.json" .models\asr\vocab.json
if not exist .models\asr\vocab.json goto :fail
call :fetch_opt "%BASE_URL%/asr_added_tokens.json"      .models\asr\added_tokens.json
if not exist .models\asr\added_tokens.json call :fetch_opt "%ASR_BASE_URL%/added_tokens.json" .models\asr\added_tokens.json
:asr_done

rem --- INT8 variants (preferred automatically when present; DOCLING_RS_FP32=1 opts out)
if %WITH_INT8%==1 (
  call :fetch_opt "%BASE_URL%/layout_heron_int8.onnx"   .models\layout_heron_int8.onnx
  call :fetch_opt "%BASE_URL%/decoder_int8.onnx"        .models\tableformer\decoder_int8.onnx
  call :fetch_opt "%BASE_URL%/decoder_kv_int8.onnx"     .models\tableformer\decoder_kv_int8.onnx
  call :fetch_opt "%BASE_URL%/decoder_kv_int8.onnx.data" .models\tableformer\decoder_kv_int8.onnx.data
  if exist .models\layout_heron_int8.onnx echo int8 models present - used by default (DOCLING_RS_FP32=1 forces full precision)
)

echo done - .models\ and .pdfium\lib populated in %CD%
exit /b 0

:fetch
if %FORCE%==1 goto :fetch_dl
if not exist "%~2" goto :fetch_dl
echo   = %~2 (already present)
exit /b 0
:fetch_dl
echo   ^> %~2
curl -fsSL %CURL_TIMEOUTS% -o "%~2" "%~1"
exit /b %errorlevel%

:fetch_opt
if %FORCE%==1 goto :fetch_opt_dl
if not exist "%~2" goto :fetch_opt_dl
echo   = %~2 (already present)
exit /b 0
:fetch_opt_dl
curl -fsSL %CURL_TIMEOUTS% -o "%~2" "%~1" >nul 2>nul
if %errorlevel%==0 (echo   ^> %~2) else (del "%~2" 2>nul)
exit /b 0

:fail
echo error: download failed - check network access and %BASE_URL%
exit /b 1

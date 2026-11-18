"""Language: en-GB. Scan synthetic QR labels using the bridge in an extracted Windows release."""
import argparse
import base64
import json
import subprocess
from pathlib import Path
from xml.etree import ElementTree

import cv2
import numpy as np


def request(bridge, action, **fields):
    result = subprocess.run(
        [str(bridge)], input=json.dumps(dict(action=action, **fields)), text=True,
        capture_output=True, timeout=30,
    )
    if result.returncode:
        raise RuntimeError(f"{action} failed: {result.stderr.strip()}")
    return json.loads(result.stdout)


def scan(encoded):
    pixels = cv2.imdecode(np.frombuffer(base64.b64decode(encoded), np.uint8), cv2.IMREAD_COLOR)
    if pixels is None:
        raise ValueError("The bridge returned an invalid preview image.")
    ink = (pixels.max(axis=2) < 90).astype(np.uint8)
    x, y, width, height = cv2.boundingRect(ink)
    if not width or not height:
        raise ValueError("The preview contains no QR code.")
    quiet = np.pad(ink[y:y + height, x:x + width], 24)
    # Keep large design previews at native size. Enlarge only printer-dot images
    # so OpenCV can distinguish modules without interpolating their edges.
    scale = max(1, round(220 / max(width, height)))
    sample = cv2.resize(255 - quiet * 255, None, fx=scale, fy=scale, interpolation=cv2.INTER_NEAREST)
    return cv2.QRCodeDetector().detectAndDecode(sample)[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("application_folder", type=Path)
    sdk = parser.parse_args().application_folder.resolve() / "sdk"
    bridge = sdk / "DymoBridge.exe"
    for width, size, payload in [(9, 5, "ITEM-A"), (19, 13, "https://example.org/B")]:
        template = base64.b64encode((sdk / "templates" / f"{width}mm.dymo").read_bytes()).decode()
        obj = dict(kind="qr", name="Extra1", value=payload, xmm=0, ymm=0, widthMm=size, heightMm=size)
        fields = dict(templateBase64=template, text="", width=width, objects=[obj], includeDesign=True)
        exported = request(bridge, "export", **fields)["labelBase64"]
        data = ElementTree.fromstring(base64.b64decode(exported)).find(".//QRCodeObject/Data")
        if data is None or data.findtext("DataString") != payload:
            raise ValueError(f"Serialised QR data differs on {width} mm tape.")
        request(bridge, "import-label", templateBase64=exported)
        reopened = dict(templateBase64=exported, width=width, text="", nativeTemplate=True,
                        preserveTemplateFormatting=True, includeDesign=True)
        for stage in [fields, reopened]:
            rendered = request(bridge, "preview", **stage)
            for image in ("preview", "designPreview"):
                if scan(rendered[image]) != payload:
                    raise ValueError(f"Wrong QR payload in {image} on {width} mm tape.")
        print(f"{width} mm: QR payload verified before export and after reopening.")


if __name__ == "__main__":
    main()

"""Language: en-GB. Scan synthetic QR labels using the bridge in an extracted Windows release."""
import argparse
import base64
import json
import subprocess
from pathlib import Path
from xml.etree import ElementTree

import cv2
import numpy as np
import zxingcpp


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
    decoded = zxingcpp.read_barcode(pixels, formats=zxingcpp.BarcodeFormat.QRCode)
    if decoded is not None:
        return decoded.text
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


def check_multiple(bridge, sdk):
    template = base64.b64encode((sdk / "templates" / "19mm.dymo").read_bytes()).decode()
    def make(name):
        obj = dict(kind="qr", name=name, value=name, xmm=0, ymm=0, widthMm=13, heightMm=13)
        result = request(bridge, "export", templateBase64=template, text="", width=19, objects=[obj])
        return ElementTree.fromstring(base64.b64decode(result["labelBase64"]))

    document = make("FIRST")
    for tag, value in [("HasFixedLength", "True"), ("FixedLengthValue", "3.5"),
                       ("InitialLength", "3.5"), ("DYMORect/Size/Width", "2.67")]:
        document.find(f".//DYMOLabel/{tag}").text = value
    first = document.find(".//QRCodeObject")
    second = make("SECOND").find(".//QRCodeObject")
    document.find(".//LabelObjects").append(second)
    x = second.find("ObjectLayout/DYMOPoint/X")
    native = dict(width=19, nativeTemplate=True, includeDesign=False)
    for x_value, covered in [("1.9", False), (first.findtext("ObjectLayout/DYMOPoint/X"), True)]:
        x.text = x_value
        native["templateBase64"] = base64.b64encode(ElementTree.tostring(document)).decode()
        preview = request(bridge, "preview", **native)
        if covered:
            if "'FIRST'" not in (preview.get("printIssue") or ""):
                raise ValueError("An obscured native QR code needs its own diagnostic.")
            try:
                request(bridge, "export", **native)
            except RuntimeError as error:
                if "FIRST" not in str(error) or "printer resolution" not in str(error):
                    raise
            else:
                raise ValueError("The obscured native code was exported.")
        elif preview.get("printIssue"):
            raise ValueError(preview["printIssue"])
        else:
            request(bridge, "export", **native)
    print("Two native QR codes checked separately, including a covered object.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("application_folder", type=Path)
    sdk = parser.parse_args().application_folder.resolve() / "sdk"
    bridge = sdk / "DymoBridge.exe"
    cases = [(6, 4.2, "ITEM-A", False), (9, 5, "ITEM-A", True),
             (9, 7, "ITEM-A", True), (12, 8.1, "ITEM-A", True),
             (19, 13, "https://example.org/B", True)]
    long_payload = "https://example.org/" + "a" * 47
    cases.extend((width, size, long_payload, width == 19)
                 for width, size in [(6, 4.2), (9, 7), (12, 8.1), (19, 14.9)])
    for width, size, payload, printable in cases:
        template = base64.b64encode((sdk / "templates" / f"{width}mm.dymo").read_bytes()).decode()
        obj = dict(kind="qr", name="Extra1", value=payload, xmm=0, ymm=0, widthMm=size, heightMm=size)
        fields = dict(templateBase64=template, text="", width=width, objects=[obj], includeDesign=True)
        preview = request(bridge, "preview", **fields)
        if not printable:
            if not preview.get("printIssue") or not preview.get("preview"):
                raise ValueError(f"Unscannable {width} mm QR needs an editable preview and diagnostic.")
            try:
                request(bridge, "export", **fields)
            except RuntimeError as error:
                if "printer resolution" not in str(error):
                    raise
            else:
                raise ValueError(f"An unscannable {width} mm QR was exported.")
            print(f"{width} mm: unreadable {len(payload)}-character QR rejected at export.")
            continue
        exported = request(bridge, "export", **fields)["labelBase64"]
        data = ElementTree.fromstring(base64.b64decode(exported)).find(".//QRCodeObject/Data")
        if data is None or data.findtext("DataString") != payload:
            raise ValueError(f"Serialised QR data differs on {width} mm tape.")
        request(bridge, "import-label", templateBase64=exported)
        reopened = dict(templateBase64=exported, width=width, text="", nativeTemplate=True,
                        preserveTemplateFormatting=True, includeDesign=True)
        for stage in [fields, reopened]:
            rendered = request(bridge, "preview", **stage)
            if rendered.get("printIssue"):
                raise ValueError(rendered["printIssue"])
            for image in ("preview", "designPreview"):
                if scan(rendered[image]) != payload:
                    raise ValueError(f"Wrong QR payload in {image} on {width} mm tape.")
        print(f"{width} mm: QR payload verified before export and after reopening.")
    check_multiple(bridge, sdk)


if __name__ == "__main__":
    main()

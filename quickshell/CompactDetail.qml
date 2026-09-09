import QtQuick
import QtQuick.Controls
import "DetailText.js" as DetailText

FocusScope {
    id: detail
    property string originalText: ""
    property string metadata: ""
    property bool expanded: false
    property color foreground: "#dddddd"
    property color mutedForeground: "#aaaaaa"
    property color background: "#202020"
    property string fontFamily: "monospace"
    property real fontSize: 12
    readonly property bool compactDetailFocusScope: true
    readonly property bool hasSelection: expanded && body.item !== null && body.item.selectedText !== ""
    readonly property string previewText: DetailText.preview(originalText)
    signal expansionRequested(bool value)
    signal copyRequested(string original)
    signal inspecting()
    signal leaveRequested()
    // Keyboard traversal must be able to bring its target into view; a control
    // focused by pointer is already where the user is looking.
    signal focusRevealed(var control)
    implicitHeight: heading.height + (expanded ? body.height + 4 : 0)
    onActiveFocusChanged: if (activeFocus) inspecting()

    // How many characters of the preview line are handed to the renderer. Zero
    // width and combining characters make every character-count estimate a
    // lower bound only, so the estimate grows until the renderer reports the
    // line truncated by width, or the whole line is laid out. Ordinary text
    // stops after the first layout; only the estimate, never the readable or
    // copyable text, changes. The step is driven by the estimate and the
    // laid-out metrics rather than onLineLaidOut, because installing that
    // handler switches Text to custom layout and disables eliding.
    property int previewLimit: DetailText.initialLimit(0, fontSize)
    function resetPreviewLimit() {
        previewLimit = DetailText.initialLimit(preview.width, fontSize)
        Qt.callLater(growPreview)
    }
    function growPreview() {
        if (previewLimit >= previewText.length || preview.truncated) return
        previewLimit = DetailText.grownLimit(previewText, previewLimit)
    }
    onPreviewLimitChanged: Qt.callLater(growPreview)
    onPreviewTextChanged: resetPreviewLimit()
    onFontSizeChanged: resetPreviewLimit()
    Component.onCompleted: resetPreviewLimit()

    // A pointer press puts focus where the user already clicked, and moving the
    // view there would interrupt a drag selection. Every other reason, chiefly
    // Tab and Backtab, may have moved focus outside the visible area.
    function revealFocus(control, reason) {
        if (reason !== Qt.MouseFocusReason && reason !== Qt.PopupFocusReason)
            focusRevealed(control)
    }

    // Child editors process selection/copy first. Unhandled keys cannot bubble
    // into the panel's action shortcuts; Tab still uses normal focus traversal.
    Keys.priority: Keys.AfterItem
    Keys.onPressed: event => {
        if (event.key === Qt.Key_Escape) {
            detail.leaveRequested()
            event.accepted = true
        } else if (event.key !== Qt.Key_Tab && event.key !== Qt.Key_Backtab) {
            event.accepted = true
        }
    }

    Item {
        id: heading
        width: parent.width
        height: Math.max(toggle.implicitHeight, metadataText.implicitHeight)
        Button {
            id: toggle
            objectName: "detailToggle"
            anchors.right: parent.right
            width: Math.min(parent.width, implicitWidth)
            text: detail.expanded ? "Collapse" : "Expand"
            font.pixelSize: detail.fontSize
            focusPolicy: Qt.StrongFocus
            Accessible.name: (detail.expanded ? "Collapse" : "Expand") + " full text"
            onActiveFocusChanged: if (activeFocus) detail.revealFocus(toggle, focusReason)
            onClicked: {
                detail.inspecting()
                detail.expansionRequested(!detail.expanded)
            }
        }
        Text {
            id: metadataText
            objectName: "detailMetadata"
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
            width: Math.min(implicitWidth, Math.max(0, heading.width - toggle.width - 12) * 0.48)
            text: detail.metadata
            textFormat: Text.PlainText
            wrapMode: Text.NoWrap
            elide: Text.ElideRight
            color: detail.foreground
            font.family: detail.fontFamily
            font.pixelSize: detail.fontSize
        }
        Text {
            id: preview
            objectName: "detailPreview"
            x: metadataText.width + (metadataText.width > 0 ? 6 : 0)
            anchors.verticalCenter: parent.verticalCenter
            width: Math.max(0, heading.width - x - toggle.width - 6)
            // Elision stays a width decision. The slice only bounds one layout
            // pass; growPreview() extends it until this Text reports itself
            // truncated, so ElideRight always chooses the visible cut.
            text: detail.previewText === "" ? "(empty text)"
                : DetailText.renderable(detail.previewText, detail.previewLimit)
            onWidthChanged: {
                detail.previewLimit = Math.max(detail.previewLimit,
                    DetailText.initialLimit(width, detail.fontSize))
                Qt.callLater(detail.growPreview)
            }
            onTruncatedChanged: Qt.callLater(detail.growPreview)
            onContentWidthChanged: Qt.callLater(detail.growPreview)
            textFormat: Text.PlainText
            maximumLineCount: 1
            wrapMode: Text.NoWrap
            elide: Text.ElideRight
            color: detail.previewText === "" ? detail.mutedForeground : detail.foreground
            font.family: detail.fontFamily
            font.pixelSize: detail.fontSize
        }
    }
    // A collapsed row must not build an editor. Instantiating one per entry
    // costs a full text layout each, and history keeps hundreds of rows.
    Loader {
        id: body
        y: heading.height + 4
        width: parent.width
        active: detail.expanded
        sourceComponent: expansion
    }
    Component {
        id: expansion
        Column {
            property alias selectedText: fullText.selectedText
            width: body.width
            spacing: 4
            Button {
                id: copyButton
                objectName: "detailCopy"
                text: "Copy full text"
                font.pixelSize: detail.fontSize
                focusPolicy: Qt.StrongFocus
                onActiveFocusChanged: if (activeFocus) detail.revealFocus(copyButton, focusReason)
                onClicked: {
                    detail.inspecting()
                    detail.copyRequested(DetailText.copySource(detail.originalText))
                }
            }
            // Wrapping inside a bounded scroller keeps arbitrarily large messages
            // usable. Word wrapping is quadratic in the length of a single line,
            // so an unbroken long line wraps at any character instead.
            ScrollView {
                width: parent.width
                height: Math.min(260, Math.max(48, fullText.implicitHeight))
                clip: true
                contentWidth: availableWidth
                TextArea {
                    id: fullText
                    objectName: "detailFullText"
                    text: detail.originalText
                    textFormat: TextEdit.PlainText
                    readOnly: true
                    selectByMouse: true
                    persistentSelection: true
                    wrapMode: DetailText.wrapsWords(detail.originalText)
                        ? TextEdit.Wrap : TextEdit.WrapAnywhere
                    color: detail.foreground
                    font.family: detail.fontFamily
                    font.pixelSize: detail.fontSize
                    background: Rectangle { color: detail.background }
                    onActiveFocusChanged: {
                        if (!activeFocus) return
                        detail.inspecting()
                        detail.revealFocus(fullText, focusReason)
                    }
                    onSelectedTextChanged: if (selectedText !== "") detail.inspecting()
                }
            }
        }
    }
}

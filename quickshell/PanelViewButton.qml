import QtQuick
import qs.Commons

// Shared extracted-view button presentation. Callers provide the palette and font.
Rectangle {
  id: button

  property string label: ""
  property bool primary: false
  required property color foreground
  required property color background
  required property color surface
  required property color accent
  required property string fontFamily
  required property real fontSize
  property color labelColor: primary && enabled ? background : foreground

  signal clicked

  implicitWidth: buttonText.implicitWidth + Style.space(18)
  width: implicitWidth
  height: buttonText.implicitHeight + Style.space(10)
  radius: 4
  color: button.primary && button.enabled ? button.accent : button.surface
  border.width: button.primary && button.enabled ? 0 : 1
  border.color: Qt.darker(button.foreground, 3)
  opacity: button.enabled ? (buttonArea.containsMouse ? 0.85 : 1.0) : 0.45

  Text {
    id: buttonText

    anchors.centerIn: parent
    width: Math.max(0, button.width - Style.space(18))
    text: button.label
    textFormat: Text.PlainText
    elide: Text.ElideRight
    color: button.labelColor
    font.family: button.fontFamily
    font.pixelSize: button.fontSize
  }

  MouseArea {
    id: buttonArea

    anchors.fill: parent
    hoverEnabled: true
    enabled: button.enabled
    onClicked: button.clicked()
  }
}

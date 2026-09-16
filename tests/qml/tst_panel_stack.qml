import QtQuick
import QtQuick.Controls
import QtTest

Item {
    id: fixture
    width: 400
    height: 300

    // Panel.qml hosts its content as an initial StackView page and later pushes
    // the discussion chat as a second page. It relies on three StackView facts
    // pinned here: (a) an item pushed with StackView.Immediate becomes
    // currentItem, is resized to fill the view, and hides the page below it;
    // (b) popping back to the initial page with pop(item, StackView.Immediate)
    // restores it as currentItem; (c) a pushed item that was declared as a
    // sibling instance (not created from a Component/delegate) is not
    // destroyed on pop and keeps its child state.

    StackView {
        id: stack
        anchors.fill: parent
        initialItem: firstPage
    }

    Item {
        id: firstPage
    }

    Item {
        id: secondPage
        TextEdit {
            id: secondPageEdit
            text: "initial"
        }
    }

    TestCase {
        name: "PanelStack"
        when: windowShown

        function test_push_sizes_hides_and_pop_preserves_state() {
            compare(stack.currentItem, firstPage)
            verify(firstPage.visible, "the initial page starts visible")

            secondPageEdit.text = "edited state"

            stack.push(secondPage, StackView.Immediate)
            compare(stack.currentItem, secondPage, "the pushed instance becomes current")
            compare(secondPage.width, stack.width, "the pushed page fills the view's width")
            compare(secondPage.height, stack.height, "the pushed page fills the view's height")
            verify(!firstPage.visible, "the page below the top is hidden")

            stack.pop(firstPage, StackView.Immediate)
            compare(stack.currentItem, firstPage, "popping restores the initial page as current")
            verify(firstPage.visible)

            verify(secondPage !== null, "the popped instance is not destroyed")
            compare(secondPageEdit.text, "edited state",
                "the popped instance keeps its child state instead of being recreated")
        }
    }
}

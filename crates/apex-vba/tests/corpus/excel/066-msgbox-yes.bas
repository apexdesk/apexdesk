' corpus-source: generated
' corpus-flagged: no
' corpus-answer: 6
' corpus-prints: cleared
Sub Main()
    Range("A1").Value = "x"
    If MsgBox("Clear A1?", vbYesNo) = vbYes Then
        Range("A1").ClearContents
        Debug.Print "cleared"
    End If
End Sub

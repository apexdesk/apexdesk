' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 255
' corpus-prints: 65280
Sub Main()
    Range("A1").Interior.Color = RGB(255, 0, 0)
    Range("A2").Interior.Color = vbGreen
    Debug.Print Range("A1").Interior.Color
    Debug.Print Range("A2").Interior.Color
End Sub

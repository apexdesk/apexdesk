' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 7
Sub AddTo(ByRef total As Long, ByVal n As Long)
    total = total + n
End Sub
Sub Main()
    Dim t As Long
    AddTo t, 3
    AddTo t, 4
    Debug.Print t
End Sub

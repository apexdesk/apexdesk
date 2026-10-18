' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
Function NextId() As Long
    Static id As Long
    id = id + 1
    NextId = id
End Function
Sub Main()
    Dim x As Long
    x = NextId
    x = NextId
    Debug.Print NextId
End Sub

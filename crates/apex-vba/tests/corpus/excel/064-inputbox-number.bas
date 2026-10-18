' corpus-source: generated
' corpus-flagged: no
' corpus-input: 7
' corpus-prints: 49
Sub Main()
    Dim n As Variant
    n = InputBox("Enter a number")
    If IsNumeric(n) Then
        Range("A1").Value = n * n
    End If
    Debug.Print Range("A1").Value
End Sub

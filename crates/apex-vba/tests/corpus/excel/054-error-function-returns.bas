' corpus-source: generated
' corpus-flagged: no
' corpus-prints: -1
' corpus-prints: 5
Function SafeDiv(a As Double, b As Double) As Double
    On Error GoTo bad
    SafeDiv = a / b
    Exit Function
bad:
    SafeDiv = -1
End Function
Sub Main()
    Debug.Print SafeDiv(5, 0)
    Debug.Print SafeDiv(10, 2)
End Sub
